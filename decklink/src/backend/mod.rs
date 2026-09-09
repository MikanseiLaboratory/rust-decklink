pub mod hardware;
pub mod mock;

use std::sync::Arc;

use crate::DeviceId;
use crate::audio::{AudioConfig, ScheduledAudioPacket};
use crate::error::Result;
use crate::frame::{FrameCompletion, ScheduledVideoFrame};
use crate::gpu::GpuBufferFactory;
use crate::mode::{DetectedFormat, DisplayMode, PixelFormat, VideoInputFlags, VideoOutputFlags};

#[derive(Clone, Debug)]
pub struct DeviceSnapshot {
    pub info: crate::device::DeviceInfo,
}

pub trait InputSink: Send + Sync {
    fn frame(&self, video: Option<crate::frame::CapturedVideoFrame>, audio: Option<crate::audio::CapturedAudioPacket>);
    fn format_changed(&self, format: DetectedFormat);
    /// Scripted / hardware end-of-stream. Default is a no-op.
    fn ended(&self) {}
}

pub trait OutputSink: Send + Sync {
    fn completed(&self, token: u64, result: FrameCompletion);
    fn stopped(&self);
    fn render_audio(&self, preroll: bool);
}

pub struct InputConfig {
    pub mode: DisplayMode,
    pub pixel_format: PixelFormat,
    pub flags: VideoInputFlags,
    pub audio: Option<AudioConfig>,
    pub time_scale: i64,
    pub gpu: Option<Arc<dyn GpuBufferFactory>>,
}

pub struct OutputConfig {
    pub mode: DisplayMode,
    pub pixel_format: PixelFormat,
    pub flags: VideoOutputFlags,
    pub audio: Option<AudioConfig>,
}

/// Backend operations run on a dedicated actor thread.
pub trait Backend {
    fn enumerate(&mut self) -> Result<Vec<DeviceSnapshot>>;
    fn open(&mut self, id: &DeviceId) -> Result<()>;
    fn display_modes(&mut self, capture: bool) -> Result<Vec<DisplayMode>>;
    fn enable_input(&mut self, config: &InputConfig, sink: Box<dyn InputSink>) -> Result<()>;
    fn start_input(&mut self) -> Result<()>;
    fn stop_input(&mut self) -> Result<()>;
    fn enable_output(&mut self, config: &OutputConfig, sink: Box<dyn OutputSink>) -> Result<()>;
    fn schedule_video(&mut self, token: u64, frame: ScheduledVideoFrame) -> Result<()>;
    fn schedule_audio(&mut self, packet: &ScheduledAudioPacket) -> Result<u32>;
    fn start_output(&mut self) -> Result<()>;
    fn stop_output(&mut self) -> Result<()>;
    fn shutdown(&mut self) -> Result<()>;
}
