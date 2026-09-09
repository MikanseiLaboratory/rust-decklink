//! Low-level C ABI for the DeckLink SDK 16.0 shim.
//!
//! This crate is intentionally thin. Owned COM pointers are represented as
//! [`Handle`] values; the safe `decklink` crate decides when to add-ref or
//! release them.

#![allow(missing_docs)]
#![allow(non_camel_case_types)]
#![allow(clippy::undocumented_unsafe_blocks)]

use std::os::raw::{c_int, c_void};
use std::ptr;

pub type HResult = i32;
pub type Handle = *mut c_void;

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_0001u32 as HResult;
pub const E_OUTOFMEMORY: HResult = 0x8000_0002u32 as HResult;
pub const E_INVALIDARG: HResult = 0x8000_0003u32 as HResult;
pub const E_NOINTERFACE: HResult = 0x8000_0004u32 as HResult;
pub const E_FAIL: HResult = 0x8000_0008u32 as HResult;
pub const E_ACCESSDENIED: HResult = 0x8000_0009u32 as HResult;

pub const ATTR_SUPPORTS_INPUT_FORMAT_DETECTION: u32 = 0x696E_6664; // 'infd'
pub const ATTR_PERSISTENT_ID: u32 = 0x7065_6964; // 'peid'
pub const ATTR_TOPOLOGICAL_ID: u32 = 0x746F_6964; // 'toid'
pub const ATTR_VIDEO_IO_SUPPORT: u32 = 0x7669_6F73; // 'vios'
pub const ATTR_DUPLEX: u32 = 0x6475_7078; // 'dupx'
pub const ATTR_MINIMUM_PREROLL_FRAMES: u32 = 0x6D70_7266; // 'mprf'
pub const ATTR_MAXIMUM_AUDIO_CHANNELS: u32 = 0x6D61_6368; // 'mach'

pub const IO_SUPPORT_CAPTURE: i64 = 1 << 0;
pub const IO_SUPPORT_PLAYBACK: i64 = 1 << 1;

pub const VIDEO_INPUT_FLAG_DEFAULT: u32 = 0;
pub const VIDEO_INPUT_ENABLE_FORMAT_DETECTION: u32 = 1 << 0;

pub const VIDEO_OUTPUT_FLAG_DEFAULT: u32 = 0;

pub const FRAME_HAS_NO_INPUT_SOURCE: u32 = 1 << 31;

pub const AUDIO_SAMPLE_RATE_48KHZ: u32 = 48_000;
pub const AUDIO_SAMPLE_TYPE_16: u32 = 16;
pub const AUDIO_SAMPLE_TYPE_32: u32 = 32;
pub const AUDIO_OUTPUT_STREAM_CONTINUOUS: u32 = 0;
pub const AUDIO_OUTPUT_STREAM_TIMESTAMPED: u32 = 2;

pub const NO_VIDEO_INPUT_CONVERSION: u32 = 0x6E6F_6E65; // 'none'
pub const NO_VIDEO_OUTPUT_CONVERSION: u32 = 0x6E6F_6E65;
pub const VIDEO_CONNECTION_UNSPECIFIED: u32 = 0;
pub const SUPPORTED_VIDEO_MODE_DEFAULT: u32 = 0;

pub const COMPLETION_COMPLETED: u32 = 0;
pub const COMPLETION_DISPLAYED_LATE: u32 = 1;
pub const COMPLETION_DROPPED: u32 = 2;
pub const COMPLETION_FLUSHED: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoInfo {
    pub width: i32,
    pub height: i32,
    pub row_bytes: i32,
    pub pixel_format: u32,
    pub flags: u32,
    pub stream_time: i64,
    pub stream_duration: i64,
    pub hardware_time: i64,
    pub hardware_duration: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioInfo {
    pub sample_frame_count: i32,
    pub packet_time: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DisplayModeInfo {
    pub mode: u32,
    pub width: i32,
    pub height: i32,
    pub frame_duration: i64,
    pub time_scale: i64,
    pub field_dominance: u32,
    pub flags: u32,
    pub name: [u8; 128],
}

impl Default for DisplayModeInfo {
    fn default() -> Self {
        Self {
            mode: 0,
            width: 0,
            height: 0,
            frame_duration: 0,
            time_scale: 0,
            field_dominance: 0,
            flags: 0,
            name: [0; 128],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InputCallbacks {
    pub ctx: *mut c_void,
    pub frame: Option<unsafe extern "C" fn(*mut c_void, Handle, Handle)>,
    pub format: Option<unsafe extern "C" fn(*mut c_void, u32, u32, i32, i32, i64, i64, u32)>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExternalBuffer {
    pub cpu: *mut c_void,
    pub size: u64,
}

pub type AllocVideoFn = unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, u32, *mut ExternalBuffer) -> HResult;
pub type FreeVideoFn = unsafe extern "C" fn(*mut c_void, *mut c_void);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OutputCallbacks {
    pub ctx: *mut c_void,
    pub completed: Option<unsafe extern "C" fn(*mut c_void, Handle, u32)>,
    pub stopped: Option<unsafe extern "C" fn(*mut c_void)>,
    pub audio_render: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
}

pub fn succeeded(hr: HResult) -> bool {
    hr >= 0
}

pub fn failed(hr: HResult) -> bool {
    hr < 0
}

extern "C" {
    pub fn rdl_initialize() -> HResult;
    pub fn rdl_uninitialize();
    pub fn rdl_hardware_available() -> i32;
    pub fn rdl_api_version(buf: *mut u8, len: usize) -> HResult;
    pub fn rdl_iterator_create(out: *mut Handle) -> HResult;
    pub fn rdl_iterator_next(iterator: Handle, device: *mut Handle) -> HResult;
    pub fn rdl_add_ref(handle: Handle);
    pub fn rdl_release(handle: Handle);
    pub fn rdl_device_model_name(device: Handle, buf: *mut u8, len: usize) -> HResult;
    pub fn rdl_device_display_name(device: Handle, buf: *mut u8, len: usize) -> HResult;
    pub fn rdl_device_profile_flag(device: Handle, attr_id: u32, value: *mut i32) -> HResult;
    pub fn rdl_device_profile_int(device: Handle, attr_id: u32, value: *mut i64) -> HResult;
    pub fn rdl_device_query_input(device: Handle, out: *mut Handle) -> HResult;
    pub fn rdl_device_query_output(device: Handle, out: *mut Handle) -> HResult;
    pub fn rdl_input_display_mode_iterator(input: Handle, out: *mut Handle) -> HResult;
    pub fn rdl_output_display_mode_iterator(output: Handle, out: *mut Handle) -> HResult;
    pub fn rdl_display_mode_iterator_next(iterator: Handle, mode: *mut Handle) -> HResult;
    pub fn rdl_display_mode_info(mode: Handle, info: *mut DisplayModeInfo) -> HResult;
    pub fn rdl_input_does_support(
        input: Handle,
        connection: u32,
        mode: u32,
        pixel_format: u32,
        conversion: u32,
        flags: u32,
        actual_mode: *mut u32,
        supported: *mut i32,
    ) -> HResult;
    pub fn rdl_input_set_callback(input: Handle, callbacks: *const InputCallbacks) -> HResult;
    pub fn rdl_input_enable_video(input: Handle, mode: u32, pixel_format: u32, flags: u32) -> HResult;
    pub fn rdl_input_enable_video_with_allocator(
        input: Handle,
        mode: u32,
        pixel_format: u32,
        flags: u32,
        alloc_ctx: *mut c_void,
        alloc: AllocVideoFn,
        free: FreeVideoFn,
    ) -> HResult;
    pub fn rdl_video_cpu_ptr(frame: Handle, ptr: *mut *mut c_void, size: *mut u64) -> HResult;
    pub fn rdl_input_disable_video(input: Handle) -> HResult;
    pub fn rdl_input_enable_audio(input: Handle, sample_rate: u32, sample_type: u32, channels: u32) -> HResult;
    pub fn rdl_input_disable_audio(input: Handle) -> HResult;
    pub fn rdl_input_start(input: Handle) -> HResult;
    pub fn rdl_input_stop(input: Handle) -> HResult;
    pub fn rdl_input_pause(input: Handle) -> HResult;
    pub fn rdl_input_flush(input: Handle) -> HResult;
    pub fn rdl_input_available_video_frames(input: Handle, count: *mut u32) -> HResult;
    pub fn rdl_video_info(frame: Handle, time_scale: i64, info: *mut VideoInfo) -> HResult;
    pub fn rdl_video_map_read(frame: Handle, data: *mut *const u8, len: *mut usize) -> HResult;
    pub fn rdl_video_unmap_read(frame: Handle) -> HResult;
    pub fn rdl_audio_info(packet: Handle, time_scale: i64, info: *mut AudioInfo) -> HResult;
    pub fn rdl_audio_bytes(packet: Handle, data: *mut *const u8, len: *mut usize) -> HResult;
    pub fn rdl_output_does_support(
        output: Handle,
        connection: u32,
        mode: u32,
        pixel_format: u32,
        conversion: u32,
        flags: u32,
        actual_mode: *mut u32,
        supported: *mut i32,
    ) -> HResult;
    pub fn rdl_output_set_callbacks(output: Handle, callbacks: *const OutputCallbacks) -> HResult;
    pub fn rdl_output_enable_video(output: Handle, mode: u32, flags: u32) -> HResult;
    pub fn rdl_output_disable_video(output: Handle) -> HResult;
    pub fn rdl_output_enable_audio(
        output: Handle,
        sample_rate: u32,
        sample_type: u32,
        channels: u32,
        stream_type: u32,
    ) -> HResult;
    pub fn rdl_output_disable_audio(output: Handle) -> HResult;
    pub fn rdl_output_row_bytes(output: Handle, pixel_format: u32, width: i32, row_bytes: *mut i32) -> HResult;
    pub fn rdl_output_create_frame(
        output: Handle,
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: u32,
        flags: u32,
        data: *const u8,
        data_len: usize,
        frame: *mut Handle,
    ) -> HResult;
    pub fn rdl_output_create_frame_from_external(
        output: Handle,
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: u32,
        flags: u32,
        cpu: *mut c_void,
        size: u64,
        frame: *mut Handle,
    ) -> HResult;
    pub fn rdl_output_schedule_video(
        output: Handle,
        frame: Handle,
        display_time: i64,
        display_duration: i64,
        time_scale: i64,
    ) -> HResult;
    pub fn rdl_output_schedule_audio(
        output: Handle,
        data: *const u8,
        sample_frame_count: u32,
        stream_time: i64,
        time_scale: i64,
        written: *mut u32,
    ) -> HResult;
    pub fn rdl_output_begin_audio_preroll(output: Handle) -> HResult;
    pub fn rdl_output_end_audio_preroll(output: Handle) -> HResult;
    pub fn rdl_output_start(output: Handle, start_time: i64, time_scale: i64, speed: f64) -> HResult;
    pub fn rdl_output_stop(output: Handle, stop_time: i64, time_scale: i64, actual: *mut i64) -> HResult;
    pub fn rdl_output_buffered_video(output: Handle, count: *mut u32) -> HResult;
    pub fn rdl_output_buffered_audio(output: Handle, count: *mut u32) -> HResult;
    pub fn rdl_output_flush_audio(output: Handle) -> HResult;
}

/// Read a NUL-terminated buffer written by the shim.
pub fn cstr_from_buf(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Convenience wrapper around [`rdl_api_version`].
pub fn api_version() -> Result<String, HResult> {
    let mut buf = [0u8; 64];
    // SAFETY: `buf` is a writable stack array owned for the duration of the call.
    let hr = unsafe { rdl_api_version(buf.as_mut_ptr(), buf.len()) };
    if failed(hr) && hr != E_NOTIMPL {
        return Err(hr);
    }
    Ok(cstr_from_buf(&buf))
}

/// Null handle helper for optional COM arguments.
pub fn null_handle() -> Handle {
    ptr::null_mut()
}
