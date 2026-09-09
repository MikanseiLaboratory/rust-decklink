use decklink::{CaptureEvent, DeckLinkContext, PixelFormat};
use futures_util::StreamExt;

fn main() -> decklink::Result<()> {
    pollster::block_on(async {
        let context = DeckLinkContext::connect();
        let device = context.first_device()?;
        let mode = device.display_modes()?.into_iter().next().expect("display mode");
        let mut capture = device
            .capture()
            .video(mode, PixelFormat::YUV_8BIT)
            .detect_format(true)
            .queue_capacity(4)
            .start()
            .await?;

        let mut remaining = 6u32;
        while let Some(event) = capture.next().await {
            match event? {
                CaptureEvent::Sample(sample) => {
                    if let Some(frame) = sample.video {
                        let bytes = frame.map_read()?.as_bytes().len();
                        println!("frame {}x{} bytes={bytes}", frame.width(), frame.height());
                    }
                    remaining = remaining.saturating_sub(1);
                }
                CaptureEvent::FormatChanged(format) => {
                    println!("format changed to {}", format.mode.name);
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
