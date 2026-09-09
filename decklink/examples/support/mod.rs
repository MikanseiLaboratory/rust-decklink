#![allow(dead_code)]

use std::env;
use std::path::PathBuf;

use decklink::{
    AudioConfig, DeckLinkContext, Device, DisplayMode, DisplayModeId, Error, ErrorKind, PixelFormat, Result,
    ScheduledAudioPacket, Time,
};

pub struct Args {
    pub require_hardware: bool,
    pub device: Option<usize>,
    pub mode: Option<String>,
    pub seconds: Option<f64>,
    pub frames: Option<u32>,
    pub out: Option<PathBuf>,
    pub audio: bool,
    pub gpu: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            require_hardware: env_truthy("DECKLINK_REQUIRE_HARDWARE"),
            device: env::var("DECKLINK_DEVICE").ok().and_then(|value| value.parse().ok()),
            mode: env::var("DECKLINK_MODE").ok().filter(|value| !value.is_empty()),
            seconds: env::var("DECKLINK_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok()),
            frames: None,
            out: None,
            audio: false,
            gpu: false,
        }
    }
}

pub fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut argv = env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--hardware" => args.require_hardware = true,
            "--audio" => args.audio = true,
            "--gpu" => args.gpu = true,
            "--device" => {
                args.device = Some(
                    parse_next(&mut argv, "--device")?
                        .parse()
                        .map_err(|_| Error::new(ErrorKind::InvalidState, "example", "--device must be a number"))?,
                );
            }
            "--mode" => args.mode = Some(parse_next(&mut argv, "--mode")?),
            "--seconds" => {
                args.seconds = Some(
                    parse_next(&mut argv, "--seconds")?
                        .parse()
                        .map_err(|_| Error::new(ErrorKind::InvalidState, "example", "--seconds must be a number"))?,
                );
            }
            "--frames" => {
                args.frames = Some(
                    parse_next(&mut argv, "--frames")?
                        .parse()
                        .map_err(|_| Error::new(ErrorKind::InvalidState, "example", "--frames must be a number"))?,
                );
            }
            "--out" => args.out = Some(PathBuf::from(parse_next(&mut argv, "--out")?)),
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidState,
                    "example",
                    format!("unknown argument {other} (see --help)"),
                ));
            }
        }
    }
    Ok(args)
}

fn parse_next(argv: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    argv.next()
        .ok_or_else(|| Error::new(ErrorKind::InvalidState, "example", format!("{flag} needs a value")))
}

fn env_truthy(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes")
    )
}

pub fn print_usage() {
    eprintln!(
        "\
options:
  --hardware          fail if the DeckLink driver / hardware shim is missing
  --device <index>    card index from list_devices (default: 0)
  --mode <name>       display mode substring, e.g. 1080p30 or Hp59
  --seconds <n>       run length (playout: omit for unlimited; capture default: 5)
  --frames <n>        stop after N video samples / scheduled frames
  --out <path>        output path for capture_frame
  --audio             enable 48 kHz stereo PCM
  --gpu               capture into wgpu shared buffers (needs --features wgpu)
  --help

env:
  DECKLINK_SDK_DIR, DECKLINK_REQUIRE_HARDWARE, DECKLINK_DEVICE, DECKLINK_MODE, DECKLINK_SECONDS"
    );
}

pub fn open_context(args: &Args) -> Result<DeckLinkContext> {
    if args.require_hardware {
        return DeckLinkContext::new().map_err(|err| {
            Error::new(
                ErrorKind::DriverNotFound,
                "example",
                format!("{err}; build with `--features hardware` and set DECKLINK_SDK_DIR to the SDK 16.0 root"),
            )
        });
    }
    let context = DeckLinkContext::connect();
    if !context.is_hardware() {
        eprintln!("using mock backend (no card or hardware feature). pass --hardware to require a device.");
    }
    Ok(context)
}

pub fn select_device(context: &DeckLinkContext, args: &Args) -> Result<Device> {
    let mut devices = context.devices()?;
    if devices.is_empty() {
        return Err(Error::new(
            ErrorKind::DriverNotFound,
            "example",
            "no DeckLink devices (is Desktop Video installed?)",
        ));
    }
    let index = args.device.unwrap_or(0);
    if index >= devices.len() {
        return Err(Error::new(
            ErrorKind::InvalidState,
            "example",
            format!("device {index} is out of range (0..{})", devices.len()),
        ));
    }
    Ok(devices.remove(index))
}

pub fn select_mode(device: &Device, args: &Args) -> Result<DisplayMode> {
    let modes = device.display_modes()?;
    if modes.is_empty() {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "example",
            "device reports no display modes",
        ));
    }
    if let Some(needle) = &args.mode {
        let needle = needle.to_ascii_lowercase();
        return modes
            .into_iter()
            .find(|mode| {
                mode.name.to_ascii_lowercase().contains(&needle) || format!("{:08x}", mode.id.0).contains(&needle)
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Unsupported,
                    "example",
                    format!("no display mode matches {needle}"),
                )
            });
    }
    Ok(modes
        .iter()
        .find(|mode| mode.id == DisplayModeId::HD1080P30)
        .or_else(|| modes.iter().find(|mode| mode.name.contains("1080")))
        .cloned()
        .unwrap_or_else(|| modes[0].clone()))
}

pub fn mode_fps(mode: &DisplayMode) -> f64 {
    mode.frame_duration.scale.get() as f64 / mode.frame_duration.value as f64
}

pub fn planned_frames(args: &Args, mode: &DisplayMode) -> Option<u32> {
    if let Some(frames) = args.frames {
        return Some(frames);
    }
    args.seconds
        .map(|seconds| (mode_fps(mode) * seconds).ceil().max(1.0) as u32)
}

pub fn capture_frames(args: &Args, mode: &DisplayMode) -> u32 {
    planned_frames(args, mode).unwrap_or_else(|| (mode_fps(mode) * 5.0).ceil().max(1.0) as u32)
}

pub fn more_frames(next: u32, total: Option<u32>) -> bool {
    total.map(|limit| next < limit).unwrap_or(true)
}

pub fn frames_label(total: Option<u32>) -> String {
    total.map(|n| n.to_string()).unwrap_or_else(|| "unlimited".into())
}

pub fn uyvy_row_bytes(width: i32) -> i32 {
    width.saturating_mul(2)
}

/// Rec.709 studio-range 8-bit Y'CbCr. Tuple is `(Y, Cb, Cr)`.
type Yuv = (u8, u8, u8);

const WHITE75: Yuv = (180, 128, 128);
const RAINBOW_HD: [Yuv; 7] = [
    WHITE75,
    (168, 44, 136),
    (145, 147, 44),
    (133, 63, 52),
    (63, 193, 204),
    (51, 109, 212),
    (28, 212, 120),
];
const WHITE100: Yuv = (235, 128, 128);
const GRAY40: Yuv = (104, 128, 128);
const GRAY15: Yuv = (49, 128, 128);
const CYAN100: Yuv = (188, 154, 16);
const YELLOW100: Yuv = (219, 16, 138);
const BLUE100: Yuv = (32, 240, 118);
const RED100: Yuv = (63, 102, 240);
const I_PIXEL: Yuv = (57, 156, 97);
const Q_PIXEL: Yuv = (44, 171, 147);
const BLACK0: Yuv = (16, 128, 128);
const BLACK2: Yuv = (20, 128, 128);
const BLACK4: Yuv = (25, 128, 128);
const NEG2: Yuv = (12, 128, 128);

/// Horizontal scroll in even pixels / frame so UYVY chroma stays aligned.
pub const SCROLL_PIXELS_PER_FRAME: i32 = 4;

/// SMPTE RP 219-2002 HD colour bars in 8-bit UYVY (`'2vuy'`).
pub fn uyvy_color_bars(width: i32, height: i32) -> Vec<u8> {
    smpte_hd_bars(width, height)
}

pub fn smpte_hd_bars(width: i32, height: i32) -> Vec<u8> {
    let width = width.max(2) as usize;
    let height = height.max(1) as usize;
    let mut bytes = vec![0u8; width * 2 * height];
    write_smpte_hd_bars(&mut bytes, width as i32, height as i32);
    bytes
}

pub fn write_uyvy_color_bars(dest: &mut [u8], width: i32, height: i32) {
    write_smpte_hd_bars(dest, width, height);
}

pub fn write_smpte_hd_bars(dest: &mut [u8], width: i32, height: i32) {
    let width = width.max(2);
    let height = height.max(1);
    let w = width as usize;
    let h = height as usize;
    let d_w = align2(width / 8);
    let top_h = align2(height * 7 / 12);
    let band_h = align2(height / 12);
    let r_w = align2((((width + 3) / 4) * 3) / 7);

    fill_uyvy_rect(dest, (w, h), (0, 0, d_w, top_h), GRAY40);
    let mut x = d_w;
    for color in RAINBOW_HD {
        fill_uyvy_rect(dest, (w, h), (x, 0, r_w, top_h), color);
        x += r_w;
    }
    fill_uyvy_rect(dest, (w, h), (x, 0, width - x, top_h), GRAY40);

    let mut y = top_h;
    fill_uyvy_rect(dest, (w, h), (0, y, d_w, band_h), CYAN100);
    x = d_w;
    fill_uyvy_rect(dest, (w, h), (x, y, r_w, band_h), I_PIXEL);
    x += r_w;
    let white_w = r_w.saturating_mul(6);
    fill_uyvy_rect(dest, (w, h), (x, y, white_w, band_h), WHITE75);
    x += white_w;
    let pluge_left = x;
    fill_uyvy_rect(dest, (w, h), (x, y, width - x, band_h), BLUE100);

    y += band_h;
    fill_uyvy_rect(dest, (w, h), (0, y, d_w, band_h), YELLOW100);
    x = d_w;
    fill_uyvy_rect(dest, (w, h), (x, y, r_w, band_h), Q_PIXEL);
    x += r_w;
    for i in (0..white_w).step_by(2) {
        let luma = (i.saturating_mul(255) / white_w.max(1)) as u8;
        fill_uyvy_rect(dest, (w, h), (x + i, y, 2, band_h), (luma, 128, 128));
    }
    x += white_w;
    fill_uyvy_rect(dest, (w, h), (x, y, width - x, band_h), RED100);

    y += band_h;
    let bottom_h = height - y;
    fill_uyvy_rect(dest, (w, h), (0, y, d_w, bottom_h), GRAY15);
    x = d_w;
    let mut span = align2(r_w * 3 / 2);
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK0);
    x += span;
    span = align2(r_w * 2);
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), WHITE100);
    x += span;
    span = align2(r_w * 5 / 6);
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK0);
    x += span;
    span = align2(r_w / 3);
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), NEG2);
    x += span;
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK0);
    x += span;
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK2);
    x += span;
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK0);
    x += span;
    fill_uyvy_rect(dest, (w, h), (x, y, span, bottom_h), BLACK4);
    x += span;
    fill_uyvy_rect(dest, (w, h), (x, y, pluge_left - x, bottom_h), BLACK0);
    x = pluge_left;
    fill_uyvy_rect(dest, (w, h), (x, y, width - x, bottom_h), GRAY15);
}

pub fn scroll_pixels(frame: u32, width: i32) -> i32 {
    let width = width.max(2) & !1;
    if width <= 0 {
        return 0;
    }
    (i32::try_from(frame)
        .unwrap_or(i32::MAX)
        .saturating_mul(SCROLL_PIXELS_PER_FRAME))
    .rem_euclid(width)
        & !1
}

pub fn blit_uyvy_hscroll(dest: &mut [u8], src: &[u8], width: i32, height: i32, scroll_px: i32) {
    let width = width.max(2) as usize;
    let height = height.max(1) as usize;
    let row_bytes = width * 2;
    let need = row_bytes.saturating_mul(height);
    if dest.len() < need || src.len() < need {
        return;
    }
    let byte_off = ((scroll_px.rem_euclid(width as i32) as usize) & !1) * 2;
    for y in 0..height {
        let row = y * row_bytes;
        if byte_off == 0 {
            dest[row..row + row_bytes].copy_from_slice(&src[row..row + row_bytes]);
        } else {
            let mid = row + row_bytes - byte_off;
            dest[row..mid].copy_from_slice(&src[row + byte_off..row + row_bytes]);
            dest[mid..row + row_bytes].copy_from_slice(&src[row..row + byte_off]);
        }
    }
}

pub fn playout_audio() -> AudioConfig {
    AudioConfig::default()
}

/// Continuous 1 kHz sine at SMPTE alignment level (-20 dBFS), 48 kHz stereo i16.
pub struct Tone {
    phase: f64,
}

impl Tone {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }

    pub fn packet(&mut self, stream_time: Time, sample_frames: usize, config: AudioConfig) -> ScheduledAudioPacket {
        let mut bytes = vec![0u8; config.byte_len(sample_frames)];
        self.fill_i16_stereo(&mut bytes);
        ScheduledAudioPacket { stream_time, bytes }
    }

    fn fill_i16_stereo(&mut self, dest: &mut [u8]) {
        const AMP: f64 = 0.1 * 32767.0;
        const STEP: f64 = 1_000.0 / 48_000.0;
        for pair in dest.chunks_exact_mut(4) {
            let sample = (AMP * (std::f64::consts::TAU * self.phase).sin()) as i16;
            let le = sample.to_le_bytes();
            pair[0] = le[0];
            pair[1] = le[1];
            pair[2] = le[0];
            pair[3] = le[1];
            self.phase += STEP;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

pub fn samples_for_video_frame(accum: &mut i64, duration: i64, video_scale: i64, sample_rate: u32) -> usize {
    if video_scale <= 0 {
        return 0;
    }
    *accum += i64::from(sample_rate) * duration;
    let count = *accum / video_scale;
    *accum %= video_scale;
    count.max(0) as usize
}

fn align2(value: i32) -> i32 {
    value.saturating_add(1) & !1
}

fn fill_uyvy_rect(buf: &mut [u8], (width, height): (usize, usize), (x, y, w, h): (i32, i32, i32, i32), yuv: Yuv) {
    if w <= 0 || h <= 0 || width < 2 {
        return;
    }
    let x0 = (x.max(0) as usize).min(width) & !1;
    let y0 = (y.max(0) as usize).min(height);
    let x1 = (x.saturating_add(w).max(0) as usize).min(width) & !1;
    let y1 = (y.saturating_add(h).max(0) as usize).min(height);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_bytes = width * 2;
    let first = y0.saturating_mul(row_bytes);
    if first >= buf.len() {
        return;
    }
    let (y_luma, cb, cr) = yuv;
    {
        let row = &mut buf[first..];
        for px in (x0..x1).step_by(2) {
            let offset = px * 2;
            if offset + 3 >= row.len() {
                break;
            }
            row[offset] = cb;
            row[offset + 1] = y_luma;
            row[offset + 2] = cr;
            row[offset + 3] = y_luma;
        }
    }
    let src = first + x0 * 2;
    let copy_len = (x1 - x0) * 2;
    if src + copy_len > buf.len() {
        return;
    }
    for row in (y0 + 1)..y1 {
        let dest = row * row_bytes + x0 * 2;
        if dest + copy_len > buf.len() {
            break;
        }
        buf.copy_within(src..src + copy_len, dest);
    }
}

pub fn pixel_format() -> PixelFormat {
    PixelFormat::YUV_8BIT
}
