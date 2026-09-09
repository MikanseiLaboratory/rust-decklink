#![allow(dead_code)]

use std::env;
use std::path::PathBuf;

use decklink::{DeckLinkContext, Device, DisplayMode, DisplayModeId, Error, ErrorKind, PixelFormat, Result};

pub struct Args {
    pub require_hardware: bool,
    pub device: Option<usize>,
    pub mode: Option<String>,
    pub seconds: f64,
    pub frames: Option<u32>,
    pub out: Option<PathBuf>,
    pub audio: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            require_hardware: env_truthy("DECKLINK_REQUIRE_HARDWARE"),
            device: env::var("DECKLINK_DEVICE").ok().and_then(|value| value.parse().ok()),
            mode: env::var("DECKLINK_MODE").ok().filter(|value| !value.is_empty()),
            seconds: env::var("DECKLINK_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5.0),
            frames: None,
            out: None,
            audio: false,
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
            "--device" => {
                args.device = Some(
                    parse_next(&mut argv, "--device")?
                        .parse()
                        .map_err(|_| Error::new(ErrorKind::InvalidState, "example", "--device must be a number"))?,
                );
            }
            "--mode" => args.mode = Some(parse_next(&mut argv, "--mode")?),
            "--seconds" => {
                args.seconds = parse_next(&mut argv, "--seconds")?
                    .parse()
                    .map_err(|_| Error::new(ErrorKind::InvalidState, "example", "--seconds must be a number"))?;
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
  --seconds <n>       run length (default: 5, or DECKLINK_SECONDS)
  --frames <n>        stop after N video samples / scheduled frames
  --out <path>        output path for capture_frame
  --audio             enable 48 kHz stereo PCM
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

pub fn planned_frames(args: &Args, mode: &DisplayMode) -> u32 {
    args.frames
        .unwrap_or_else(|| (mode_fps(mode) * args.seconds).ceil().max(1.0) as u32)
}

pub fn uyvy_row_bytes(width: i32) -> i32 {
    width.saturating_mul(2)
}

/// 75% colour bars in 8-bit UYVY (`PixelFormat::YUV_8BIT` / `'2vuy'`).
pub fn uyvy_color_bars(width: i32, height: i32) -> Vec<u8> {
    let width = width.max(2) as usize;
    let height = height.max(1) as usize;
    let row_bytes = width * 2;
    let mut bytes = vec![0u8; row_bytes * height];
    const BARS: [(u8, u8, u8); 8] = [
        (180, 128, 128),
        (168, 44, 136),
        (145, 147, 44),
        (133, 63, 52),
        (63, 193, 204),
        (51, 109, 212),
        (28, 212, 120),
        (16, 128, 128),
    ];
    for y in 0..height {
        for x in (0..width).step_by(2) {
            let bar = BARS[x * BARS.len() / width];
            let offset = y * row_bytes + x * 2;
            bytes[offset] = bar.1;
            bytes[offset + 1] = bar.0;
            bytes[offset + 2] = bar.2;
            bytes[offset + 3] = bar.0;
        }
    }
    bytes
}

pub fn pixel_format() -> PixelFormat {
    PixelFormat::YUV_8BIT
}
