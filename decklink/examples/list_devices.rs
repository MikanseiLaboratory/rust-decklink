#[path = "support/mod.rs"]
mod support;

use support::{open_context, parse_args};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    let context = open_context(&args)?;
    println!(
        "backend={} api={}",
        if context.is_hardware() { "hardware" } else { "mock" },
        context.api_version()?
    );
    let devices = context.devices()?;
    if devices.is_empty() {
        println!("no devices");
        return Ok(());
    }
    for device in devices {
        let info = device.info();
        println!(
            "[{}] {} ({}) persistent={:?} topo={:?} capture={} playback={} format_detect={} audio_ch={:?} preroll={:?}",
            device.index(),
            info.display_name,
            info.model_name,
            info.id.persistent_id,
            info.id.topological_id,
            info.supports_capture,
            info.supports_playback,
            info.supports_format_detection,
            info.maximum_audio_channels,
            info.minimum_preroll_frames
        );
        for mode in device.display_modes()? {
            println!(
                "  {} {}x{} {:.3}fps scale={}/{} dominance={:?}",
                mode.name,
                mode.width,
                mode.height,
                support::mode_fps(&mode),
                mode.frame_duration.value,
                mode.frame_duration.scale,
                mode.field_dominance
            );
        }
    }
    Ok(())
}
