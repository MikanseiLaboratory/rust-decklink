#[path = "support/mod.rs"]
mod support;

use std::time::Instant;

use decklink::{AudioConfig, CaptureEvent};
use futures_util::StreamExt;
use support::{open_context, parse_args, pixel_format, capture_frames, select_device, select_mode};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        let context = open_context(&args)?;
        let device = select_device(&context, &args)?;
        let mode = select_mode(&device, &args)?;
        let target = capture_frames(&args, &mode);
        println!(
            "stats {} {} target_frames={target} expected_fps={:.3}",
            device.info().display_name,
            mode.name,
            support::mode_fps(&mode)
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
        let started = Instant::now();
        let mut samples = 0u32;
        let mut with_source = 0u32;
        let mut no_source = 0u32;
        let mut audio_packets = 0u32;
        let mut format_changes = 0u32;
        let mut overflows = 0u64;

        while samples < target {
            match capture.next().await {
                Some(Ok(CaptureEvent::Sample(sample))) => {
                    samples += 1;
                    if let Some(frame) = sample.video {
                        if frame.has_input_source() {
                            with_source += 1;
                        } else {
                            no_source += 1;
                        }
                    }
                    if sample.audio.is_some() {
                        audio_packets += 1;
                    }
                }
                Some(Ok(CaptureEvent::FormatChanged(format))) => {
                    format_changes += 1;
                    println!("format changed to {}", format.mode.name);
                }
                Some(Ok(CaptureEvent::Overflow(info))) => {
                    overflows += info.dropped;
                    println!("overflow dropped={}", info.dropped);
                }
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }

        let elapsed = started.elapsed().as_secs_f64().max(1e-6);
        println!(
            "samples={samples} with_source={with_source} no_source={no_source} \
             audio={audio_packets} format_changes={format_changes} overflow_dropped={overflows} \
             measured_fps={:.3} elapsed_s={elapsed:.3}",
            samples as f64 / elapsed
        );
        if no_source > 0 && with_source == 0 {
            eprintln!("no input source on any frame. check the SDI/HDMI cable and the generator.");
        }
        capture.shutdown().await
    })
}
