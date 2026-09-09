//! wgpu 30 helpers. DeckLink always sees lockable host pages; GPU import is optional.

#![allow(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "macos")]
use std::ffi::c_void;
#[cfg(target_os = "macos")]
use std::ptr::NonNull;

#[cfg(not(target_os = "macos"))]
use crate::error::Result;
#[cfg(target_os = "macos")]
use crate::error::{Error, ErrorKind, Result};
#[cfg(target_os = "macos")]
use crate::gpu::NativeGpuHandle;
use crate::gpu::{AllocatedGpuBuffer, CpuSharedFactory, GpuBackend, GpuBufferFactory, GpuBufferRequest};

/// Shared-memory factory backed by the same wgpu device eiviz already owns.
#[derive(Clone)]
pub struct WgpuSharedFactory {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    device: wgpu::Device,
    backend: wgpu::Backend,
}

impl WgpuSharedFactory {
    pub fn new(device: wgpu::Device, backend: wgpu::Backend) -> Self {
        Self { device, backend }
    }

    /// wgpu adapter this factory was built from. DeckLink storage may still be CPU.
    pub fn wgpu_backend(&self) -> wgpu::Backend {
        self.backend
    }
}

impl GpuBufferFactory for WgpuSharedFactory {
    fn backend(&self) -> GpuBackend {
        // Report the memory DeckLink actually receives, not the wgpu adapter.
        #[cfg(target_os = "macos")]
        if self.backend == wgpu::Backend::Metal {
            return GpuBackend::Metal;
        }
        GpuBackend::Cpu
    }

    fn allocate(&self, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
        // D3D12 / Vulkan mapped heaps are not DeckLink-DMA-safe (capture is black).
        // Metal shared is UMA, so it can be the primary pointer on macOS.
        #[cfg(target_os = "macos")]
        if self.backend == wgpu::Backend::Metal {
            if let Ok(buffer) = allocate_metal(&self.device, request) {
                return Ok(buffer);
            }
        }
        CpuSharedFactory.allocate(request)
    }
}

#[cfg(target_os = "macos")]
fn allocate_metal(device: &wgpu::Device, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
    use objc2_metal::{MTLDevice, MTLResourceOptions};

    let mtl = unsafe {
        device
            .as_hal::<wgpu::hal::api::Metal>()
            .map(|hal| hal.raw_device().clone())
    }
    .ok_or_else(|| Error::new(ErrorKind::Unsupported, "wgpu", "device is not Metal"))?;

    let size = request.byte_size.max(1) as usize;
    let raw_buf = mtl
        .newBufferWithLength_options(size, MTLResourceOptions::StorageModeShared)
        .ok_or_else(|| Error::new(ErrorKind::Sdk, "mtl_buffer", "newBufferWithLength failed"))?;
    let cpu_ptr = raw_buf.contents().as_ptr().cast::<u8>();
    let native = objc2::rc::Retained::as_ptr(&raw_buf) as *mut c_void;
    let handle = NativeGpuHandle::MetalBuffer(NonNull::new(native).expect("mtl buffer"));
    let hal = unsafe { wgpu::hal::metal::Device::buffer_from_raw(&raw_buf, size as u64) };
    let wgpu_buffer = unsafe {
        device.create_buffer_from_hal::<wgpu::hal::api::Metal>(
            hal,
            &wgpu::BufferDescriptor {
                label: Some("decklink metal shared"),
                size: size as u64,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
    };
    Ok(AllocatedGpuBuffer {
        backend: GpuBackend::Metal,
        cpu_ptr,
        size,
        handle,
        request,
        wgpu_buffer: Some(wgpu_buffer),
        wgpu_texture: None,
        drop: Some(Box::new(move || {
            drop(raw_buf);
        })),
    })
}
