#[path = "support/mod.rs"]
mod support;

use decklink::{
    CpuSharedFactory, FrameCompletion, GpuBufferFactory, GpuBufferRequest, PlayoutEvent, ScheduledVideoFrame, Time,
};
use futures_util::StreamExt;
use support::{
    blit_uyvy_hscroll, frames_label, more_frames, open_context, parse_args, pixel_format, planned_frames,
    playout_audio, samples_for_video_frame, scroll_pixels, select_device, select_mode, smpte_hd_bars, uyvy_row_bytes,
    Tone,
};

fn main() -> decklink::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        let context = open_context(&args)?;
        let device = select_device(&context, &args)?;
        let mode = select_mode(&device, &args)?;
        let total = planned_frames(&args, &mode);
        let row_bytes = uyvy_row_bytes(mode.width);
        let byte_size = (row_bytes as u32).saturating_mul(mode.height.max(1) as u32);
        let request = GpuBufferRequest {
            width: mode.width.max(0) as u32,
            height: mode.height.max(0) as u32,
            row_bytes: row_bytes as u32,
            byte_size,
            pixel_format: pixel_format(),
        };
        let factory = CpuSharedFactory;
        let pattern = smpte_hd_bars(mode.width, mode.height);
        let audio = playout_audio();
        let mut tone = Tone::new();

        const WINDOW: usize = 8;
        let mut slots = Vec::new();
        for _ in 0..WINDOW {
            let allocated = factory.allocate(request)?;
            let access = allocated.access(request, factory.backend());
            slots.push((allocated, access));
        }
        println!(
            "SMPTE HD bars + 1 kHz + scroll → {} {} {}x{} frames={} slots={WINDOW}",
            device.info().display_name,
            mode.name,
            mode.width,
            mode.height,
            frames_label(total)
        );

        let mut playout = device
            .playout()
            .video(mode.clone(), pixel_format())
            .audio(audio)
            .start()
            .await?;
        let scale = mode.frame_duration.scale.get();
        let duration = mode.frame_duration.value;
        let mut next = 0u32;
        let mut in_flight = 0u32;
        let mut completed = 0u32;
        let mut sample_accum = 0i64;

        while more_frames(next, total) || in_flight > 0 {
            while in_flight < WINDOW as u32 && more_frames(next, total) {
                let slot = (next as usize) % WINDOW;
                {
                    let allocated = &slots[slot].0;
                    // SAFETY: the factory keeps `cpu_ptr` valid for `allocated.size` bytes,
                    // and this slot is only rewritten after its previous frame completed.
                    let dest = unsafe { std::slice::from_raw_parts_mut(allocated.cpu_ptr, allocated.size) };
                    blit_uyvy_hscroll(dest, &pattern, mode.width, mode.height, scroll_pixels(next, mode.width));
                }
                let gpu = slots[slot].1.clone();
                let display_time = Time::new(duration.saturating_mul(i64::from(next)), scale)?;
                playout
                    .schedule_video(ScheduledVideoFrame {
                        width: mode.width,
                        height: mode.height,
                        row_bytes,
                        pixel_format: pixel_format(),
                        flags: 0,
                        display_time,
                        display_duration: mode.frame_duration,
                        bytes: Vec::new(),
                        gpu: Some(gpu),
                    })
                    .await?;
                let samples = samples_for_video_frame(&mut sample_accum, duration, scale, audio.sample_rate);
                if samples > 0 {
                    let _ = playout
                        .schedule_audio(tone.packet(display_time, samples, audio))
                        .await?;
                }
                next += 1;
                in_flight += 1;
            }
            match playout.next().await {
                Some(Ok(PlayoutEvent::FrameCompleted { result, .. })) => {
                    in_flight = in_flight.saturating_sub(1);
                    if result == FrameCompletion::Completed {
                        completed += 1;
                    }
                }
                Some(Ok(PlayoutEvent::Stopped)) | None => break,
                Some(Err(err)) => return Err(err),
                Some(Ok(_)) => {}
            }
        }
        println!("completed={completed}");
        let result = playout.shutdown().await;
        drop(slots);
        result
    })
}
