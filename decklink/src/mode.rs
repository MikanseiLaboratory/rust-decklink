use crate::time::Time;

/// FourCC display mode identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DisplayModeId(pub u32);

impl DisplayModeId {
    pub const HD1080P25: Self = Self(0x4870_3235); // 'Hp25'
    pub const HD1080P2997: Self = Self(0x4870_3239); // 'Hp29'
    pub const HD1080P30: Self = Self(0x4870_3330); // 'Hp30'
    pub const HD1080P50: Self = Self(0x4870_3530); // 'Hp50'
    pub const HD1080P5994: Self = Self(0x4870_3539); // 'Hp59'
    pub const HD1080I50: Self = Self(0x4869_3530); // 'Hi50'
    pub const HD1080I5994: Self = Self(0x4869_3539); // 'Hi59'
    pub const HD720P50: Self = Self(0x6870_3530); // 'hp50'
    pub const HD720P5994: Self = Self(0x6870_3539); // 'hp59'
    pub const NTSC: Self = Self(0x6E74_7363); // 'ntsc'
    pub const PAL: Self = Self(0x7061_6C20); // 'pal '
    pub const UNKNOWN: Self = Self(0x6975_6E6B); // 'iunk'
}

/// Pixel format FourCC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelFormat(pub u32);

impl PixelFormat {
    pub const UNSPECIFIED: Self = Self(0);
    pub const YUV_8BIT: Self = Self(0x3276_7579); // '2vuy'
    pub const YUV_10BIT: Self = Self(0x7632_3130); // 'v210'
    pub const ARGB_8BIT: Self = Self(32);
    pub const BGRA_8BIT: Self = Self(0x4247_5241); // 'BGRA'
}

/// Field dominance FourCC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldDominance(pub u32);

impl FieldDominance {
    pub const UNKNOWN: Self = Self(0);
    pub const LOWER_FIRST: Self = Self(0x6C6F_7772);
    pub const UPPER_FIRST: Self = Self(0x7570_7072);
    pub const PROGRESSIVE: Self = Self(0x7072_6F67);
    pub const PSF: Self = Self(0x7073_6620);
}

/// Display mode metadata copied out of `IDeckLinkDisplayMode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub id: DisplayModeId,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub frame_duration: Time,
    pub field_dominance: FieldDominance,
    pub flags: u32,
}

impl DisplayMode {
    pub fn hd1080p30() -> Self {
        Self {
            id: DisplayModeId::HD1080P30,
            name: "1080p30".into(),
            width: 1920,
            height: 1080,
            frame_duration: Time::new(1001, 30_000).expect("constant"),
            field_dominance: FieldDominance::PROGRESSIVE,
            flags: 0,
        }
    }
}

/// Detected input signal flags from `VideoInputFormatChanged`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedFormat {
    pub events: u32,
    pub mode: DisplayMode,
    pub flags: u32,
}

/// Video input enable flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoInputFlags(pub u32);

impl VideoInputFlags {
    pub const DEFAULT: Self = Self(0);
    pub const FORMAT_DETECTION: Self = Self(decklink_sys::VIDEO_INPUT_ENABLE_FORMAT_DETECTION);

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Video output enable flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoOutputFlags(pub u32);

impl VideoOutputFlags {
    pub const DEFAULT: Self = Self(0);
}
