use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;

use crate::actor::ActorHandle;
use crate::audio::{AudioConfig, CapturedAudioPacket};
use crate::backend::{InputConfig, InputSink};
use crate::device::Device;
use crate::error::{Error, ErrorKind, Result};
use crate::frame::CapturedVideoFrame;
use crate::gpu::GpuBufferFactory;
use crate::mode::{DetectedFormat, DisplayMode, PixelFormat, VideoInputFlags};
use crate::queue::{EventQueue, EventStream, OverflowInfo, OverflowPolicy};
use crate::state::SessionState;

/// Video plus optional audio delivered by one callback invocation.
#[derive(Debug)]
pub struct CaptureSample {
    pub video: Option<CapturedVideoFrame>,
    pub audio: Option<CapturedAudioPacket>,
}

/// Capture stream item.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CaptureEvent {
    Sample(CaptureSample),
    FormatChanged(DetectedFormat),
    Overflow(OverflowInfo),
}

pub type CaptureError = Error;

/// Builder for an async capture session.
pub struct CaptureBuilder {
    device: Device,
    mode: Option<DisplayMode>,
    pixel_format: PixelFormat,
    flags: VideoInputFlags,
    audio: Option<AudioConfig>,
    queue_capacity: usize,
    overflow: OverflowPolicy,
    gpu: Option<Arc<dyn GpuBufferFactory>>,
}

impl CaptureBuilder {
    pub fn new(device: Device) -> Self {
        Self {
            device,
            mode: None,
            pixel_format: PixelFormat::YUV_8BIT,
            flags: VideoInputFlags::DEFAULT,
            audio: None,
            queue_capacity: 4,
            overflow: OverflowPolicy::DropOldest,
            gpu: None,
        }
    }

    pub fn video(mut self, mode: DisplayMode, pixel_format: PixelFormat) -> Self {
        self.mode = Some(mode);
        self.pixel_format = pixel_format;
        self
    }

    pub fn detect_format(mut self, enable: bool) -> Self {
        if enable {
            self.flags = self.flags.union(VideoInputFlags::FORMAT_DETECTION);
        }
        self
    }

    pub fn audio(mut self, audio: AudioConfig) -> Self {
        self.audio = Some(audio);
        self
    }

    pub fn queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = capacity.max(1);
        self
    }

    pub fn overflow(mut self, policy: OverflowPolicy) -> Self {
        self.overflow = policy;
        self
    }

    pub fn gpu_buffers(mut self, factory: Arc<dyn GpuBufferFactory>) -> Self {
        self.gpu = Some(factory);
        self
    }

    pub async fn start(self) -> Result<Capture> {
        let mode = match self.mode {
            Some(mode) => mode,
            None => self
                .device
                .display_modes()?
                .into_iter()
                .next()
                .ok_or_else(|| Error::new(ErrorKind::Unsupported, "capture", "no display modes"))?,
        };
        let actor = crate::context::actor_for(&self.device)?;
        actor.open(self.device.info.id.clone()).await?;
        let time_scale = mode.frame_duration.scale.get();
        let queue = EventQueue::new(self.queue_capacity, self.overflow);
        let sink = Box::new(CaptureSink {
            queue: Arc::clone(&queue),
        });
        actor
            .enable_input(
                InputConfig {
                    mode,
                    pixel_format: self.pixel_format,
                    flags: self.flags,
                    audio: self.audio,
                    time_scale,
                    gpu: self.gpu,
                },
                sink,
            )
            .await?;
        actor.start_input().await?;
        Ok(Capture {
            actor,
            events: EventStream::new(queue),
            overflow_reported: false,
            state: SessionState::Running,
            stopped: false,
        })
    }
}

struct CaptureSink {
    queue: Arc<EventQueue<Result<CaptureEvent>>>,
}

impl InputSink for CaptureSink {
    fn frame(&self, video: Option<CapturedVideoFrame>, audio: Option<CapturedAudioPacket>) {
        let event = Ok(CaptureEvent::Sample(CaptureSample { video, audio }));
        self.push(event);
    }

    fn format_changed(&self, format: DetectedFormat) {
        self.push(Ok(CaptureEvent::FormatChanged(format)));
    }
}

impl CaptureSink {
    fn push(&self, event: Result<CaptureEvent>) {
        let _ = self.queue.try_push(event);
    }
}

/// Async capture session. Drop requests stop but does not wait.
pub struct Capture {
    actor: ActorHandle,
    events: EventStream<Result<CaptureEvent>>,
    overflow_reported: bool,
    state: SessionState,
    stopped: bool,
}

impl Capture {
    pub fn state(&self) -> SessionState {
        self.state
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.state = SessionState::Stopping;
        self.actor.stop_input().await?;
        self.events.queue().close();
        self.stopped = true;
        self.state = SessionState::Stopped;
        Ok(())
    }
}

impl Stream for Capture {
    type Item = Result<CaptureEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if !this.overflow_reported {
            let info = this.events.queue().overflow_info();
            if info.dropped > 0 {
                this.overflow_reported = true;
                return Poll::Ready(Some(Ok(CaptureEvent::Overflow(info))));
            }
        }
        Pin::new(&mut this.events).poll_next(cx)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if !self.stopped {
            self.actor.stop_input_now();
            self.events.queue().close();
        }
    }
}
