#![allow(clippy::undocumented_unsafe_blocks)]

use std::marker::PhantomData;

use crate::error::{Error, ErrorKind, Result};
use crate::gpu::GpuFrameAccess;
use crate::mode::PixelFormat;
use crate::time::Time;

impl std::fmt::Debug for CapturedVideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedVideoFrame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("pixel_format", &self.pixel_format())
            .finish()
    }
}

/// Owned or hardware-backed captured video frame.
///
/// The type is `Send` so a callback thread can hand it to an async task after
/// `AddRef`. It is intentionally `!Sync`; mapped buffers are not shared.
pub struct CapturedVideoFrame {
    inner: VideoInner,
}

enum VideoInner {
    Owned(OwnedVideo),
    Hardware(HardwareVideo),
}

struct OwnedVideo {
    width: i32,
    height: i32,
    row_bytes: i32,
    pixel_format: PixelFormat,
    flags: u32,
    stream_time: Option<Time>,
    hardware_time: Option<Time>,
    bytes: Vec<u8>,
    gpu: Option<GpuFrameAccess>,
}

struct HardwareVideo {
    handle: decklink_sys::Handle,
    info: OwnedVideo,
    gpu: Option<GpuFrameAccess>,
}

// DeckLink samples pass AddRef'd frames across threads. Concurrent aliasing is
// not documented, so we do not implement Sync.
unsafe impl Send for HardwareVideo {}

impl CapturedVideoFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn owned(
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: PixelFormat,
        flags: u32,
        stream_time: Option<Time>,
        hardware_time: Option<Time>,
        bytes: Vec<u8>,
    ) -> Self {
        Self::owned_with_gpu(
            width,
            height,
            row_bytes,
            pixel_format,
            flags,
            stream_time,
            hardware_time,
            bytes,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn owned_with_gpu(
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: PixelFormat,
        flags: u32,
        stream_time: Option<Time>,
        hardware_time: Option<Time>,
        bytes: Vec<u8>,
        gpu: Option<GpuFrameAccess>,
    ) -> Self {
        Self {
            inner: VideoInner::Owned(OwnedVideo {
                width,
                height,
                row_bytes,
                pixel_format,
                flags,
                stream_time,
                hardware_time,
                bytes,
                gpu,
            }),
        }
    }

    pub(crate) fn from_hardware(
        handle: decklink_sys::Handle,
        time_scale: i64,
        gpu: Option<&crate::gpu::GpuRegistry>,
    ) -> Result<Self> {
        let mut info = decklink_sys::VideoInfo::default();
        Error::check("video_info", unsafe {
            decklink_sys::rdl_video_info(handle, time_scale, &mut info)
        })?;
        let stream_time = Time::new(info.stream_time, time_scale).ok();
        let hardware_time = Time::new(info.hardware_time, time_scale).ok();
        let gpu = {
            let mut ptr = std::ptr::null_mut();
            let mut size = 0u64;
            let hr = unsafe { decklink_sys::rdl_video_cpu_ptr(handle, &mut ptr, &mut size) };
            if decklink_sys::succeeded(hr) {
                gpu.and_then(|registry| registry.lookup(ptr.cast()))
            } else {
                None
            }
        };
        Ok(Self {
            inner: VideoInner::Hardware(HardwareVideo {
                handle,
                gpu,
                info: OwnedVideo {
                    width: info.width,
                    height: info.height,
                    row_bytes: info.row_bytes,
                    pixel_format: PixelFormat(info.pixel_format),
                    flags: info.flags,
                    stream_time,
                    hardware_time,
                    bytes: Vec::new(),
                    gpu: None,
                },
            }),
        })
    }

    pub fn gpu(&self) -> Option<&GpuFrameAccess> {
        match &self.inner {
            VideoInner::Owned(owned) => owned.gpu.as_ref(),
            VideoInner::Hardware(hw) => hw.gpu.as_ref(),
        }
    }

    pub fn width(&self) -> i32 {
        self.meta().width
    }

    pub fn height(&self) -> i32 {
        self.meta().height
    }

    pub fn row_bytes(&self) -> i32 {
        self.meta().row_bytes
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.meta().pixel_format
    }

    pub fn flags(&self) -> u32 {
        self.meta().flags
    }

    pub fn has_input_source(&self) -> bool {
        self.flags() & decklink_sys::FRAME_HAS_NO_INPUT_SOURCE == 0
    }

    pub fn stream_time(&self) -> Option<Time> {
        self.meta().stream_time
    }

    pub fn hardware_time(&self) -> Option<Time> {
        self.meta().hardware_time
    }

    /// CPU-visible view of the frame bytes. The returned guard must not outlive
    /// this frame; dropping it ends SDK buffer access.
    pub fn map_read(&self) -> Result<VideoReadGuard<'_>> {
        match &self.inner {
            VideoInner::Owned(owned) => Ok(VideoReadGuard {
                ptr: owned.bytes.as_ptr(),
                len: owned.bytes.len(),
                hardware: None,
                _marker: PhantomData,
            }),
            VideoInner::Hardware(hw) => {
                let mut ptr = std::ptr::null();
                let mut len = 0usize;
                Error::check("video_map_read", unsafe {
                    decklink_sys::rdl_video_map_read(hw.handle, &mut ptr, &mut len)
                })?;
                Ok(VideoReadGuard {
                    ptr,
                    len,
                    hardware: Some(hw.handle),
                    _marker: PhantomData,
                })
            }
        }
    }

    fn meta(&self) -> &OwnedVideo {
        match &self.inner {
            VideoInner::Owned(owned) => owned,
            VideoInner::Hardware(hw) => &hw.info,
        }
    }
}

impl Drop for CapturedVideoFrame {
    fn drop(&mut self) {
        if let VideoInner::Hardware(hw) = &self.inner {
            unsafe { decklink_sys::rdl_release(hw.handle) };
        }
    }
}

/// Borrowed mapping of a video frame. `!Send + !Sync` because it pins SDK access.
pub struct VideoReadGuard<'a> {
    ptr: *const u8,
    len: usize,
    hardware: Option<decklink_sys::Handle>,
    _marker: PhantomData<&'a CapturedVideoFrame>,
}

impl VideoReadGuard<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            // SAFETY: `map_read` keeps this pointer valid until the guard is dropped.
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }
}

impl Drop for VideoReadGuard<'_> {
    fn drop(&mut self) {
        if let Some(handle) = self.hardware {
            let _ = unsafe { decklink_sys::rdl_video_unmap_read(handle) };
        }
    }
}

/// User-owned frame scheduled for playback.
///
/// The hardware backend wraps `bytes` or `gpu.cpu_ptr` with
/// `CreateVideoFrameWithBuffer` and keeps this value until
/// `ScheduledFrameCompleted`. If the GPU wrote the buffer through a command
/// queue, wait for that fence before scheduling so DeckLink does not DMA unread
/// pixels.
#[derive(Clone, Debug)]
pub struct ScheduledVideoFrame {
    pub width: i32,
    pub height: i32,
    pub row_bytes: i32,
    pub pixel_format: PixelFormat,
    pub flags: u32,
    pub display_time: Time,
    pub display_duration: Time,
    pub bytes: Vec<u8>,
    pub gpu: Option<GpuFrameAccess>,
}

impl ScheduledVideoFrame {
    pub fn validate(&self) -> Result<()> {
        if self.gpu.is_some() {
            return Ok(());
        }
        if self.bytes.len() < (self.row_bytes as usize).saturating_mul(self.height as usize) {
            return Err(Error::new(
                ErrorKind::InvalidState,
                "schedule_video",
                "frame buffer is smaller than row_bytes * height",
            ));
        }
        Ok(())
    }
}

/// Completion reported by `IDeckLinkVideoOutputCallback`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameCompletion {
    Completed,
    DisplayedLate,
    Dropped,
    Flushed,
}

impl FrameCompletion {
    pub fn from_raw(value: u32) -> Self {
        match value {
            decklink_sys::COMPLETION_DISPLAYED_LATE => Self::DisplayedLate,
            decklink_sys::COMPLETION_DROPPED => Self::Dropped,
            decklink_sys::COMPLETION_FLUSHED => Self::Flushed,
            _ => Self::Completed,
        }
    }
}
