use crate::error::Result;
use crate::mode::DisplayMode;

/// Stable-enough identity for a DeckLink node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId {
    pub persistent_id: Option<u64>,
    pub topological_id: Option<u32>,
    pub display_name: String,
}

/// Device capability snapshot taken at enumeration time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub model_name: String,
    pub display_name: String,
    pub supports_capture: bool,
    pub supports_playback: bool,
    pub supports_format_detection: bool,
    pub maximum_audio_channels: Option<u32>,
    pub minimum_preroll_frames: Option<u32>,
}

/// Opened device handle that can start capture or playout.
#[derive(Clone, Debug)]
pub struct Device {
    pub info: DeviceInfo,
    pub(crate) index: usize,
    pub(crate) actor: crate::actor::ActorHandle,
}

impl Device {
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn capture(&self) -> crate::capture::CaptureBuilder {
        crate::capture::CaptureBuilder::new(self.clone())
    }

    pub fn playout(&self) -> crate::playout::PlayoutBuilder {
        crate::playout::PlayoutBuilder::new(self.clone())
    }

    pub fn display_modes(&self) -> Result<Vec<DisplayMode>> {
        crate::context::list_display_modes(self)
    }
}
