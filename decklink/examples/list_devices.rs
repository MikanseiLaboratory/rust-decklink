use decklink::DeckLinkContext;

fn main() -> decklink::Result<()> {
    let context = DeckLinkContext::connect();
    println!("API {}", context.api_version()?);
    for device in context.devices()? {
        let info = device.info();
        println!(
            "{} ({}) capture={} playback={} format_detect={}",
            info.display_name,
            info.model_name,
            info.supports_capture,
            info.supports_playback,
            info.supports_format_detection
        );
        for mode in device.display_modes()? {
            println!(
                "  {} {}x{} {}/{}",
                mode.name, mode.width, mode.height, mode.frame_duration.value, mode.frame_duration.scale
            );
        }
    }
    Ok(())
}
