use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::audio::{AudioConfig, CapturedAudioPacket, ScheduledAudioPacket};
use crate::backend::{Backend, DeviceSnapshot, InputConfig, InputSink, OutputConfig, OutputSink};
use crate::device::{DeviceId, DeviceInfo};
use crate::error::{Error, ErrorKind, Result};
use crate::frame::{CapturedVideoFrame, FrameCompletion, ScheduledVideoFrame};
use crate::mode::{DetectedFormat, DisplayMode, PixelFormat};
use crate::time::Time;

/// Scripted capture event used by tests and examples.
#[derive(Clone, Debug)]
pub enum MockCaptureEvent {
    Sample {
        video: Option<MockVideo>,
        audio: Option<MockAudio>,
    },
    FormatChanged(DetectedFormat),
    Disconnect,
}

#[derive(Clone, Debug)]
pub struct MockVideo {
    pub width: i32,
    pub height: i32,
    pub row_bytes: i32,
    pub pixel_format: PixelFormat,
    pub flags: u32,
    pub bytes: Vec<u8>,
    pub stream_time: Option<Time>,
}

#[derive(Clone, Debug)]
pub struct MockAudio {
    pub config: AudioConfig,
    pub bytes: Vec<u8>,
    pub packet_time: Option<Time>,
}

/// In-memory DeckLink stand-in. Safe to use without the SDK.
#[derive(Clone, Debug)]
pub struct MockWorld {
    pub devices: Vec<DeviceInfo>,
    pub modes: Vec<DisplayMode>,
    pub capture_events: Vec<MockCaptureEvent>,
    pub complete_immediately: bool,
    /// Delay between scripted capture events. `None` bursts them immediately.
    pub capture_interval: Option<Duration>,
}

impl MockWorld {
    pub fn demo() -> Self {
        let mode = DisplayMode::hd1080p30();
        let row_bytes = mode.width * 2;
        let bytes = vec![0x80; (row_bytes * mode.height) as usize];
        Self {
            devices: vec![DeviceInfo {
                id: DeviceId {
                    persistent_id: Some(1),
                    topological_id: Some(1),
                    display_name: "Mock DeckLink".into(),
                },
                model_name: "Mock".into(),
                display_name: "Mock DeckLink".into(),
                supports_capture: true,
                supports_playback: true,
                supports_format_detection: true,
                maximum_audio_channels: Some(16),
                minimum_preroll_frames: Some(3),
            }],
            modes: vec![mode.clone()],
            capture_events: vec![MockCaptureEvent::Sample {
                video: Some(MockVideo {
                    width: mode.width,
                    height: mode.height,
                    row_bytes,
                    pixel_format: PixelFormat::YUV_8BIT,
                    flags: 0,
                    bytes,
                    stream_time: Time::new(0, 30_000).ok(),
                }),
                audio: None,
            }],
            complete_immediately: true,
            capture_interval: Some(Duration::from_millis(1)),
        }
    }
}

pub struct MockBackend {
    world: MockWorld,
    stop: Arc<AtomicBool>,
    output_sink: Option<Arc<dyn OutputSink>>,
    output_audio: Option<AudioConfig>,
    next_token: AtomicU64,
    scheduled: Mutex<Vec<u64>>,
}

impl MockBackend {
    pub fn new(world: MockWorld) -> Self {
        Self {
            world,
            stop: Arc::new(AtomicBool::new(false)),
            output_sink: None,
            output_audio: None,
            next_token: AtomicU64::new(1),
            scheduled: Mutex::new(Vec::new()),
        }
    }
}

fn deliver_mock_events(
    sink: &dyn InputSink,
    events: &[MockCaptureEvent],
    stop: &AtomicBool,
    interval: Option<Duration>,
) {
    for event in events {
        if stop.load(Ordering::Acquire) {
            break;
        }
        match event {
            MockCaptureEvent::Sample { video, audio } => {
                let video = video.as_ref().map(|frame| {
                    CapturedVideoFrame::owned(
                        frame.width,
                        frame.height,
                        frame.row_bytes,
                        frame.pixel_format,
                        frame.flags,
                        frame.stream_time,
                        None,
                        frame.bytes.clone(),
                    )
                });
                let audio = audio.as_ref().and_then(|packet| {
                    CapturedAudioPacket::owned(packet.config, packet.packet_time, packet.bytes.clone()).ok()
                });
                sink.frame(video, audio);
            }
            MockCaptureEvent::FormatChanged(format) => sink.format_changed(format.clone()),
            MockCaptureEvent::Disconnect => {
                stop.store(true, Ordering::Release);
                break;
            }
        }
        if let Some(interval) = interval {
            thread::sleep(interval);
        }
    }
}

impl Backend for MockBackend {
    fn enumerate(&mut self) -> Result<Vec<DeviceSnapshot>> {
        Ok(self
            .world
            .devices
            .iter()
            .cloned()
            .map(|info| DeviceSnapshot { info })
            .collect())
    }

    fn open(&mut self, id: &DeviceId) -> Result<()> {
        if self.world.devices.iter().any(|device| &device.id == id) {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::DriverNotFound, "open", "mock device not found"))
        }
    }

    fn display_modes(&mut self, _capture: bool) -> Result<Vec<DisplayMode>> {
        Ok(self.world.modes.clone())
    }

    fn enable_input(&mut self, _config: &InputConfig, sink: Box<dyn InputSink>) -> Result<()> {
        let events = self.world.capture_events.clone();
        let interval = self.world.capture_interval;
        let stop = Arc::clone(&self.stop);
        // Burst scripts run on this thread so overflow tests cannot lose a race
        // against the consumer and hang on an open, empty queue.
        if interval.is_none() {
            deliver_mock_events(sink.as_ref(), &events, &stop, None);
            sink.ended();
            return Ok(());
        }
        thread::Builder::new()
            .name("decklink-mock-capture".into())
            .spawn(move || {
                deliver_mock_events(sink.as_ref(), &events, &stop, interval);
                sink.ended();
            })
            .map_err(|err| Error::new(ErrorKind::Sdk, "mock_capture", err.to_string()))?;
        Ok(())
    }

    fn start_input(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop_input(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        Ok(())
    }

    fn enable_output(&mut self, config: &OutputConfig, sink: Box<dyn OutputSink>) -> Result<()> {
        let _ = config.pixel_format;
        self.output_audio = config.audio;
        self.output_sink = Some(Arc::from(sink));
        Ok(())
    }

    fn schedule_video(&mut self, token: u64, frame: &ScheduledVideoFrame) -> Result<()> {
        frame.validate()?;
        self.scheduled.lock().expect("scheduled").push(token);
        if self.world.complete_immediately {
            if let Some(sink) = &self.output_sink {
                sink.completed(token, FrameCompletion::Completed);
            }
        }
        let _ = self.next_token.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn schedule_audio(&mut self, packet: &ScheduledAudioPacket) -> Result<u32> {
        let bytes_per_frame = self.output_audio.unwrap_or_default().bytes_per_frame().max(1);
        Ok((packet.bytes.len() / bytes_per_frame) as u32)
    }

    fn start_output(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop_output(&mut self) -> Result<()> {
        if let Some(sink) = &self.output_sink {
            sink.stopped();
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        self.stop_input()?;
        self.stop_output()
    }
}
