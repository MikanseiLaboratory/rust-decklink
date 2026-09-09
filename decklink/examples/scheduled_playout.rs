#[path = "support/mod.rs"]
mod support;

use decklink::{FrameCompletion, PlayoutEvent, ScheduledVideoFrame, Time};
use futures_util::StreamExt;
use support::{
    open_context, parse_args, pixel_format, planned_frames, select_device, select_mode, uyvy_color_bars, uyvy_row_bytes,
};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        let context = open_context(&args)?;
        let device = select_device(&context, &args)?;
        let mode = select_mode(&device, &args)?;
        let total = planned_frames(&args, &mode);
        let row_bytes = uyvy_row_bytes(mode.width);
        let bytes = uyvy_color_bars(mode.width, mode.height);
        println!(
            "playout {} {} {}x{} frames={total} bars={} bytes",
            device.info().display_name,
            mode.name,
            mode.width,
            mode.height,
            bytes.len()
        );

        let mut playout = device.playout().video(mode.clone(), pixel_format()).start().await?;

        let scale = mode.frame_duration.scale.get();
        let duration = mode.frame_duration.value;
        let mut next = 0u32;
        let mut in_flight = 0u32;
        let mut completed = 0u32;
        let mut late = 0u32;
        let mut dropped = 0u32;
        let mut flushed = 0u32;
        const WINDOW: u32 = 8;

        while next < total || in_flight > 0 {
            while in_flight < WINDOW && next < total {
                let frame = ScheduledVideoFrame {
                    width: mode.width,
                    height: mode.height,
                    row_bytes,
                    pixel_format: pixel_format(),
                    flags: 0,
                    display_time: Time::new(duration.saturating_mul(i64::from(next)), scale)?,
                    display_duration: mode.frame_duration,
                    bytes: bytes.clone(),
                    gpu: None,
                };
                let token = playout.schedule_video(frame).await?;
                if next == 0 {
                    println!("first token={token}");
                }
                next += 1;
                in_flight += 1;
            }

            match playout.next().await {
                Some(Ok(PlayoutEvent::FrameCompleted { result, .. })) => {
                    in_flight = in_flight.saturating_sub(1);
                    match result {
                        FrameCompletion::Completed => completed += 1,
                        FrameCompletion::DisplayedLate => late += 1,
                        FrameCompletion::Dropped => dropped += 1,
                        FrameCompletion::Flushed => flushed += 1,
                    }
                }
                Some(Ok(PlayoutEvent::Stopped)) => break,
                Some(Ok(other)) => println!("{other:?}"),
                Some(Err(err)) => return Err(err),
                None => break,
            }
        }

        println!("completed={completed} late={late} dropped={dropped} flushed={flushed}");
        playout.shutdown().await
    })
}
