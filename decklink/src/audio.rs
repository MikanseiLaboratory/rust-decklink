#![allow(clippy::undocumented_unsafe_blocks)]

use crate::error::{Error, ErrorKind, Result};
use crate::time::Time;

/// PCM sample size used by DeckLink audio I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleType {
    Integer16,
    Integer32,
}

impl SampleType {
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::Integer16 => 2,
            Self::Integer32 => 4,
        }
    }

    pub fn raw(self) -> u32 {
        match self {
            Self::Integer16 => decklink_sys::AUDIO_SAMPLE_TYPE_16,
            Self::Integer32 => decklink_sys::AUDIO_SAMPLE_TYPE_32,
        }
    }

    pub fn from_raw(value: u32) -> Self {
        if value == decklink_sys::AUDIO_SAMPLE_TYPE_32 {
            Self::Integer32
        } else {
            Self::Integer16
        }
    }
}

/// Audio configuration shared by capture and playout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub sample_type: SampleType,
    pub channels: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: decklink_sys::AUDIO_SAMPLE_RATE_48KHZ,
            sample_type: SampleType::Integer16,
            channels: 2,
        }
    }
}

impl AudioConfig {
    pub fn bytes_per_frame(self) -> usize {
        self.channels as usize * self.sample_type.bytes_per_sample()
    }

    pub fn byte_len(self, sample_frames: usize) -> usize {
        sample_frames * self.bytes_per_frame()
    }
}

impl std::fmt::Debug for CapturedAudioPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturedAudioPacket")
            .field("sample_frames", &self.sample_frames())
            .field("config", &self.config())
            .finish()
    }
}

/// Captured audio packet. `Send` after AddRef; not `Sync`.
pub struct CapturedAudioPacket {
    inner: AudioInner,
}

enum AudioInner {
    Owned(OwnedAudio),
    Hardware(HardwareAudio),
}

struct OwnedAudio {
    config: AudioConfig,
    sample_frames: usize,
    packet_time: Option<Time>,
    bytes: Vec<u8>,
}

struct HardwareAudio {
    handle: decklink_sys::Handle,
    config: AudioConfig,
    sample_frames: usize,
    packet_time: Option<Time>,
}

unsafe impl Send for HardwareAudio {}

impl CapturedAudioPacket {
    pub fn owned(config: AudioConfig, packet_time: Option<Time>, bytes: Vec<u8>) -> Result<Self> {
        if config.bytes_per_frame() == 0 || bytes.len() % config.bytes_per_frame() != 0 {
            return Err(Error::new(
                ErrorKind::InvalidState,
                "audio",
                "audio buffer is not a multiple of sample-frame size",
            ));
        }
        Ok(Self {
            inner: AudioInner::Owned(OwnedAudio {
                sample_frames: bytes.len() / config.bytes_per_frame(),
                config,
                packet_time,
                bytes,
            }),
        })
    }

    pub(crate) fn from_hardware(handle: decklink_sys::Handle, config: AudioConfig, time_scale: i64) -> Result<Self> {
        let mut info = decklink_sys::AudioInfo::default();
        Error::check("audio_info", unsafe {
            decklink_sys::rdl_audio_info(handle, time_scale, &mut info)
        })?;
        Ok(Self {
            inner: AudioInner::Hardware(HardwareAudio {
                handle,
                config,
                sample_frames: info.sample_frame_count.max(0) as usize,
                packet_time: Time::new(info.packet_time, time_scale).ok(),
            }),
        })
    }

    pub fn sample_frames(&self) -> usize {
        match &self.inner {
            AudioInner::Owned(owned) => owned.sample_frames,
            AudioInner::Hardware(hw) => hw.sample_frames,
        }
    }

    pub fn packet_time(&self) -> Option<Time> {
        match &self.inner {
            AudioInner::Owned(owned) => owned.packet_time,
            AudioInner::Hardware(hw) => hw.packet_time,
        }
    }

    pub fn config(&self) -> AudioConfig {
        match &self.inner {
            AudioInner::Owned(owned) => owned.config,
            AudioInner::Hardware(hw) => hw.config,
        }
    }

    pub fn as_bytes(&self) -> Result<&[u8]> {
        match &self.inner {
            AudioInner::Owned(owned) => Ok(&owned.bytes),
            AudioInner::Hardware(hw) => {
                let mut ptr = std::ptr::null();
                let mut len = 0usize;
                Error::check("audio_bytes", unsafe {
                    decklink_sys::rdl_audio_bytes(hw.handle, &mut ptr, &mut len)
                })?;
                let byte_len = hw.config.byte_len(hw.sample_frames);
                if ptr.is_null() {
                    Ok(&[])
                } else {
                    // SAFETY: the packet still holds the AddRef'd buffer for this borrow.
                    Ok(unsafe { std::slice::from_raw_parts(ptr, byte_len) })
                }
            }
        }
    }
}

impl Drop for CapturedAudioPacket {
    fn drop(&mut self) {
        if let AudioInner::Hardware(hw) = &self.inner {
            unsafe { decklink_sys::rdl_release(hw.handle) };
        }
    }
}

/// Audio packet scheduled for playback.
#[derive(Clone, Debug)]
pub struct ScheduledAudioPacket {
    pub stream_time: Time,
    pub bytes: Vec<u8>,
}

/// Remainder after a partial `ScheduleAudioSamples` write.
pub fn remaining_audio(bytes: &[u8], written_frames: u32, config: AudioConfig) -> Vec<u8> {
    let offset = config.byte_len(written_frames as usize);
    if offset >= bytes.len() {
        Vec::new()
    } else {
        bytes[offset..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_write_keeps_tail() {
        let config = AudioConfig::default();
        let bytes = vec![0u8; config.byte_len(8)];
        let rest = remaining_audio(&bytes, 3, config);
        assert_eq!(rest.len(), config.byte_len(5));
    }
}
