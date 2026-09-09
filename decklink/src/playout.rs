use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use futures_core::Stream;
use futures_sink::Sink;

use crate::actor::ActorHandle;
use crate::audio::{AudioConfig, ScheduledAudioPacket};
use crate::backend::{OutputConfig, OutputSink};
use crate::device::Device;
use crate::error::{Error, ErrorKind, Result};
use crate::frame::{FrameCompletion, ScheduledVideoFrame};
use crate::mode::{DisplayMode, PixelFormat, VideoOutputFlags};
use crate::queue::{EventQueue, EventStream, OverflowPolicy};
use crate::state::SessionState;

/// Events from scheduled playback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayoutEvent {
    FrameCompleted { token: u64, result: FrameCompletion },
    AudioRender { preroll: bool },
    Stopped,
}

pub type PlayoutError = Error;

pub struct PlayoutBuilder {
    device: Device,
    mode: Option<DisplayMode>,
    pixel_format: PixelFormat,
    flags: VideoOutputFlags,
    audio: Option<AudioConfig>,
}

impl PlayoutBuilder {
    pub fn new(device: Device) -> Self {
        Self {
            device,
            mode: None,
            pixel_format: PixelFormat::YUV_8BIT,
            flags: VideoOutputFlags::DEFAULT,
            audio: None,
        }
    }

    pub fn video(mut self, mode: DisplayMode, pixel_format: PixelFormat) -> Self {
        self.mode = Some(mode);
        self.pixel_format = pixel_format;
        self
    }

    pub fn audio(mut self, audio: AudioConfig) -> Self {
        self.audio = Some(audio);
        self
    }

    pub async fn start(self) -> Result<Playout> {
        let mode = match self.mode {
            Some(mode) => mode,
            None => self
                .device
                .display_modes()?
                .into_iter()
                .next()
                .ok_or_else(|| Error::new(ErrorKind::Unsupported, "playout", "no display modes"))?,
        };
        let actor = crate::context::actor_for(&self.device)?;
        actor.open(self.device.info.id.clone()).await?;
        let events = EventQueue::new(32, OverflowPolicy::DropOldest);
        let sink = Box::new(PlayoutSink {
            queue: Arc::clone(&events),
        });
        actor
            .enable_output(
                OutputConfig {
                    mode,
                    pixel_format: self.pixel_format,
                    flags: self.flags,
                    audio: self.audio,
                },
                sink,
            )
            .await?;
        Ok(Playout {
            actor,
            events: EventStream::new(events),
            next_token: Arc::new(AtomicU64::new(1)),
            state: SessionState::Running,
            stopped: false,
        })
    }
}

struct PlayoutSink {
    queue: Arc<EventQueue<Result<PlayoutEvent>>>,
}

impl OutputSink for PlayoutSink {
    fn completed(&self, token: u64, result: FrameCompletion) {
        let _ = self.queue.try_push(Ok(PlayoutEvent::FrameCompleted { token, result }));
    }

    fn stopped(&self) {
        let _ = self.queue.try_push(Ok(PlayoutEvent::Stopped));
        self.queue.close();
    }

    fn render_audio(&self, preroll: bool) {
        let _ = self.queue.try_push(Ok(PlayoutEvent::AudioRender { preroll }));
    }
}

/// Scheduled playback session with video/audio sinks and an event stream.
pub struct Playout {
    actor: ActorHandle,
    events: EventStream<Result<PlayoutEvent>>,
    next_token: Arc<AtomicU64>,
    state: SessionState,
    stopped: bool,
}

impl Playout {
    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn video(&self) -> VideoSender {
        VideoSender {
            actor: self.actor.clone(),
            next_token: Arc::clone(&self.next_token),
            pending: None,
        }
    }

    pub fn audio(&self) -> AudioSender {
        AudioSender {
            actor: self.actor.clone(),
            pending: None,
        }
    }

    pub fn events(&self) -> EventStream<Result<PlayoutEvent>> {
        EventStream::new(Arc::clone(self.events.queue()))
    }

    pub async fn schedule_video(&self, frame: ScheduledVideoFrame) -> Result<u64> {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        self.actor.schedule_video(token, frame).await?;
        Ok(token)
    }

    pub async fn schedule_audio(&self, packet: ScheduledAudioPacket) -> Result<u32> {
        self.actor.schedule_audio(packet).await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.state = SessionState::Stopping;
        self.actor.stop_output().await?;
        self.events.queue().close();
        self.stopped = true;
        self.state = SessionState::Stopped;
        Ok(())
    }
}

impl Stream for Playout {
    type Item = Result<PlayoutEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.events).poll_next(cx)
    }
}

impl Drop for Playout {
    fn drop(&mut self) {
        if !self.stopped {
            self.actor.stop_output_now();
            self.events.queue().close();
        }
    }
}

/// `Sink` for scheduled video frames.
pub struct VideoSender {
    actor: ActorHandle,
    next_token: Arc<AtomicU64>,
    pending: Option<futures_channel::oneshot::Receiver<Result<()>>>,
}

impl Sink<ScheduledVideoFrame> for VideoSender {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        if let Some(pending) = &mut self.pending {
            match Pin::new(pending).poll(cx) {
                Poll::Ready(Ok(Ok(_))) => {
                    self.pending = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Ok(Err(err))) => Poll::Ready(Err(err)),
                Poll::Ready(Err(_)) => Poll::Ready(Err(Error::new(
                    ErrorKind::Cancelled,
                    "schedule_video",
                    "actor dropped the reply",
                ))),
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: ScheduledVideoFrame) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::new(
                ErrorKind::InvalidState,
                "schedule_video",
                "sink is not ready",
            ));
        }
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        self.pending = Some(self.actor.begin_schedule_video(token, item)?);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_ready(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }
}

/// `Sink` for scheduled audio packets.
pub struct AudioSender {
    actor: ActorHandle,
    pending: Option<futures_channel::oneshot::Receiver<Result<u32>>>,
}

impl Sink<ScheduledAudioPacket> for AudioSender {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        if let Some(pending) = &mut self.pending {
            match Pin::new(pending).poll(cx) {
                Poll::Ready(Ok(Ok(_))) => {
                    self.pending = None;
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Ok(Err(err))) => Poll::Ready(Err(err)),
                Poll::Ready(Err(_)) => Poll::Ready(Err(Error::new(
                    ErrorKind::Cancelled,
                    "schedule_audio",
                    "actor dropped the reply",
                ))),
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: ScheduledAudioPacket) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::new(
                ErrorKind::InvalidState,
                "schedule_audio",
                "sink is not ready",
            ));
        }
        self.pending = Some(self.actor.begin_schedule_audio(item)?);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_ready(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }
}
