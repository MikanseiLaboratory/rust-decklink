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
            "writing one {} {}x{} frame to {} gpu={}",
            mode.name,
            mode.width,
            mode.height,
            out.display(),
            args.gpu
        );

        let mut builder = device
            .capture()
            .video(mode.clone(), pixel_format())
            .detect_format(true)
            .queue_capacity(4);
        builder = attach_gpu(builder, args.gpu)?;

        let mut capture = builder.start().await?;
        println!("capture started, waiting for a frame with input source");

        let mut written = false;
        let mut skipped = 0u32;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while let Some(event) = capture.next().await {
            match event? {
                CaptureEvent::Sample(sample) => {
                    if let Some(frame) = sample.video {
                        if !frame.has_input_source() {
                            skipped += 1;
                            if skipped == 1 || skipped % 60 == 0 {
                                println!("no input source yet skipped={skipped} flags=0x{:08x}", frame.flags());
                            }
                            if std::time::Instant::now() > deadline {
                                return Err(decklink::Error::new(
                                    decklink::ErrorKind::Disconnected,
                                    "example",
                                    format!("no input source after {skipped} frames"),
                                ));
                            }
                            continue;
                        }
                        let bytes = frame_bytes(&frame)?;
                        fs::write(&out, &bytes).map_err(|err| {
                            decklink::Error::new(
                                decklink::ErrorKind::Sdk,
                                "example",
                                format!("failed to write {}: {err}", out.display()),
                            )
                        })?;
                        println!(
                            "wrote {} bytes source={} gpu={} (ffplay -f rawvideo -pixel_format uyvy422 -video_size {}x{} {})",
                            bytes.len(),
                            frame.has_input_source(),
                            frame.gpu().map(|gpu| format!("{:?}", gpu.backend)).unwrap_or_else(|| "none".into()),
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

fn attach_gpu(builder: decklink::CaptureBuilder, gpu: bool) -> decklink::Result<decklink::CaptureBuilder> {
    if !gpu {
        return Ok(builder);
    }
    #[cfg(feature = "wgpu")]
    {
        use std::sync::Arc;

        use decklink::{GpuBufferFactory, WgpuSharedFactory};

        let (_instance, gpu_device, _queue, backend) = request_wgpu()?;
        let factory = Arc::new(WgpuSharedFactory::new(gpu_device, backend));
        println!("wgpu backend={backend:?} factory={:?}", factory.backend());
        Ok(builder.gpu_buffers(factory))
    }
    #[cfg(not(feature = "wgpu"))]
    {
        let _ = builder;
        Err(decklink::Error::new(
            decklink::ErrorKind::Unsupported,
            "example",
            "--gpu needs `--features wgpu`",
        ))
    }
}

fn frame_bytes(frame: &decklink::CapturedVideoFrame) -> decklink::Result<Vec<u8>> {
    Ok(frame.map_read()?.as_bytes().to_vec())
}

#[cfg(feature = "wgpu")]
fn request_wgpu() -> decklink::Result<(wgpu::Instance, wgpu::Device, wgpu::Queue, wgpu::Backend)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::METAL,
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                shader_compiler: wgpu::Dx12Compiler::Fxc,
                ..Default::default()
            },
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|err| {
        decklink::Error::new(
            decklink::ErrorKind::Unsupported,
            "wgpu",
            format!("no GPU adapter: {err}"),
        )
    })?;
    let info = adapter.get_info();
    println!("adapter={} driver={}", info.name, info.driver);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .map_err(|err| decklink::Error::new(decklink::ErrorKind::Sdk, "wgpu", err.to_string()))?;
    Ok((instance, device, queue, info.backend))
}
