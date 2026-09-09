#[path = "support/mod.rs"]
mod support;

use std::fs;
use std::path::PathBuf;

use decklink::CaptureEvent;
use futures_util::StreamExt;
use support::{open_context, parse_args, pixel_format, select_device, select_mode};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        let context = open_context(&args)?;
        let device = select_device(&context, &args)?;
        let mode = select_mode(&device, &args)?;
        let out = args.out.clone().unwrap_or_else(|| PathBuf::from("frame.uyvy"));
        println!(
            "writing one {} {}x{} frame to {}",
            mode.name,
            mode.width,
            mode.height,
            out.display()
        );

        let mut capture = device
            .capture()
            .video(mode.clone(), pixel_format())
            .detect_format(true)
            .queue_capacity(4)
            .start()
            .await?;

        let mut written = false;
        while let Some(event) = capture.next().await {
            match event? {
                CaptureEvent::Sample(sample) => {
                    if let Some(frame) = sample.video {
                        let bytes = frame.map_read()?.as_bytes().to_vec();
                        fs::write(&out, &bytes).map_err(|err| {
                            decklink::Error::new(
                                decklink::ErrorKind::Sdk,
                                "example",
                                format!("failed to write {}: {err}", out.display()),
                            )
                        })?;
                        println!(
                            "wrote {} bytes source={} (ffplay -f rawvideo -pixel_format uyvy422 -video_size {}x{} {})",
                            bytes.len(),
                            frame.has_input_source(),
                            frame.width(),
                            frame.height(),
                            out.display()
                        );
                        written = true;
                        break;
                    }
                }
                CaptureEvent::FormatChanged(format) => {
                    println!("format changed to {}", format.mode.name);
                }
                CaptureEvent::Overflow(info) => {
                    println!("overflow dropped={}", info.dropped);
                }
            }
        }
        capture.shutdown().await?;
        if written {
            Ok(())
        } else {
            Err(decklink::Error::new(
                decklink::ErrorKind::Disconnected,
                "example",
                "capture ended before a video frame arrived",
            ))
        }
    })
}
