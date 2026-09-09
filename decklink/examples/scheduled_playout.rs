use decklink::{DeckLinkContext, PixelFormat, PlayoutEvent, ScheduledVideoFrame, Time};
use futures_util::StreamExt;

fn main() -> decklink::Result<()> {
    pollster::block_on(async {
        let context = DeckLinkContext::connect();
        let device = context.first_device()?;
        let mode = device.display_modes()?.into_iter().next().expect("display mode");
        let mut playout = device
            .playout()
            .video(mode.clone(), PixelFormat::YUV_8BIT)
            .start()
            .await?;

        let row_bytes = mode.width * 2;
        let bytes = vec![0x10; (row_bytes * mode.height) as usize];
        let token = playout
            .schedule_video(ScheduledVideoFrame {
                width: mode.width,
                height: mode.height,
                row_bytes,
                pixel_format: PixelFormat::YUV_8BIT,
                flags: 0,
                display_time: Time::new(0, mode.frame_duration.scale.get())?,
                display_duration: mode.frame_duration,
                bytes,
            })
            .await?;
        println!("scheduled token={token}");

        if let Some(event) = playout.next().await {
            match event? {
                PlayoutEvent::FrameCompleted { token, result } => {
                    println!("completed token={token} result={result:?}");
                }
                other => println!("{other:?}"),
            }
        }
        playout.shutdown().await
    })
}
