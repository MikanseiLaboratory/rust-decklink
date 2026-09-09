#[path = "support/mod.rs"]
mod support;

use decklink::{AudioConfig, CaptureEvent};
use futures_util::StreamExt;
use support::{open_context, parse_args, pixel_format, capture_frames, select_device, select_mode};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        let context = open_context(&args)?;
        let device = select_device(&context, &args)?;
        let mode = select_mode(&device, &args)?;
        let frames = capture_frames(&args, &mode);
        println!(
            "capture {} [{}] {} {}x{} frames={frames}",
            device.info().display_name,
            device.index(),
            mode.name,
            mode.width,
            mode.height
        );

        let mut builder = device
            .capture()
            .video(mode, pixel_format())
            .detect_format(true)
            .queue_capacity(8);
        if args.audio {
            builder = builder.audio(AudioConfig::default());
        }
        let mut capture = builder.start().await?;

        let mut remaining = frames;
        while let Some(event) = capture.next().await {
            match event? {
                CaptureEvent::Sample(sample) => {
                    if let Some(frame) = sample.video {
                        let bytes = frame.map_read()?.as_bytes().len();
                        println!(
                            "frame {}x{} bytes={bytes} source={} t={:?}",
                            frame.width(),
                            frame.height(),
                            frame.has_input_source(),
                            frame.stream_time()
                        );
                    }
                    if let Some(audio) = sample.audio {
                        println!("audio frames={}", audio.sample_frames());
                    }
                    remaining = remaining.saturating_sub(1);
                }
                CaptureEvent::FormatChanged(format) => {
                    println!("format changed to {} flags={}", format.mode.name, format.flags);
                }
                CaptureEvent::Overflow(info) => {
                    println!("overflow dropped={}", info.dropped);
                }
            }
            if remaining == 0 {
                break;
            }
        }
        capture.shutdown().await
    })
}
