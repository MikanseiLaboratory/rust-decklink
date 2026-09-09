//! Safe, runtime-agnostic bindings for Blackmagic DeckLink capture and playout.
//!
//! The public async surface is [`futures_core::Stream`] / [`futures_sink::Sink`].
//! DeckLink COM objects stay on a dedicated actor thread.

mod actor;
mod audio;
mod backend;
mod capture;
mod context;
mod device;
mod error;
mod frame;
mod mode;
mod playout;
mod queue;
mod state;
mod time;

pub use audio::{remaining_audio, AudioConfig, CapturedAudioPacket, SampleType, ScheduledAudioPacket};
pub use backend::mock::{MockAudio, MockCaptureEvent, MockVideo, MockWorld};
pub use capture::{Capture, CaptureBuilder, CaptureError, CaptureEvent, CaptureSample};
pub use context::DeckLinkContext;
pub use device::{Device, DeviceId, DeviceInfo};
pub use error::{Error, ErrorKind, Hresult, Result};
pub use frame::{CapturedVideoFrame, FrameCompletion, ScheduledVideoFrame, VideoReadGuard};
pub use mode::{
    DetectedFormat, DisplayMode, DisplayModeId, FieldDominance, PixelFormat, VideoInputFlags, VideoOutputFlags,
};
pub use playout::{AudioSender, Playout, PlayoutBuilder, PlayoutError, PlayoutEvent, VideoSender};
pub use queue::{OverflowInfo, OverflowPolicy};
pub use state::SessionState;
pub use time::Time;

/// Installed driver / shim version string.
pub fn api_version() -> Result<String> {
    DeckLinkContext::connect().api_version()
}
