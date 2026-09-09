#[path = "support/mod.rs"]
mod support;

use decklink::{FrameCompletion, PlayoutEvent, ScheduledVideoFrame, Time};
use futures_util::StreamExt;
use support::{
    frames_label, more_frames, open_context, parse_args, pixel_format, planned_frames, select_device, select_mode,
    uyvy_color_bars, uyvy_row_bytes,
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
            "playout {} {} {}x{} frames={} bars={} bytes",
            device.info().display_name,
            mode.name,
            mode.width,
            mode.height,
            frames_label(total),
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

        while more_frames(next, total) || in_flight > 0 {
            while in_flight < WINDOW && more_frames(next, total) {
                let frame = ScheduledVideoFrame::from_bytes(
                    mode.width,
                    mode.height,
                    row_bytes,
                    pixel_format(),
                    0,
                    Time::new(duration.saturating_mul(i64::from(next)), scale)?,
                    mode.frame_duration,
                    bytes.clone(),
                );
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
