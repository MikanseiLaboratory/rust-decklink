//! Shared GPU buffers for DeckLink capture / playout.
//!
//! DeckLink DMA needs a lockable CPU pointer (`IDeckLinkVideoBuffer::GetBytes`).
//! Buffers are page-aligned host memory (VirtualAlloc / 4 KB). wgpu may import
//! the same pages; do not pass GPU BAR / `GPU_UPLOAD` addresses to DeckLink.

#![allow(clippy::undocumented_unsafe_blocks)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::error::{Error, ErrorKind, Result};
use crate::mode::PixelFormat;

mod pinned;
#[cfg(feature = "wgpu")]
mod wgpu_alloc;

#[cfg(feature = "wgpu")]
pub use wgpu_alloc::WgpuSharedFactory;

/// Which native API produced a shared buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    Cpu,
    D3D12,
    Metal,
    Vulkan,
}

/// Native resource eiviz can import through wgpu HAL.
#[derive(Clone, Copy, Debug)]
pub enum NativeGpuHandle {
    None,
    /// `ID3D12Resource*` (buffer). Linear layout, DeckLink row bytes.
    D3D12Resource(NonNull<c_void>),
    /// `MTLBuffer*`.
    MetalBuffer(NonNull<c_void>),
    /// `MTLTexture*`.
    MetalTexture(NonNull<c_void>),
    /// `VkBuffer` handle.
    VulkanBuffer(u64),
}

unsafe impl Send for NativeGpuHandle {}
unsafe impl Sync for NativeGpuHandle {}

/// What DeckLink asked the allocator to create.
#[derive(Clone, Copy, Debug)]
pub struct GpuBufferRequest {
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub byte_size: u32,
    pub pixel_format: PixelFormat,
}

/// Allocation returned to the DeckLink allocator callback.
pub struct AllocatedGpuBuffer {
    pub cpu_ptr: *mut u8,
    pub size: usize,
    pub handle: NativeGpuHandle,
    #[cfg(feature = "wgpu")]
    pub wgpu_buffer: Option<wgpu::Buffer>,
    #[cfg(feature = "wgpu")]
    pub wgpu_texture: Option<wgpu::Texture>,
    _cpu: Option<Vec<u8>>,
    drop: Option<Box<dyn FnOnce() + Send>>,
}

unsafe impl Send for AllocatedGpuBuffer {}

impl AllocatedGpuBuffer {
    pub fn access(&self, request: GpuBufferRequest, backend: GpuBackend) -> GpuFrameAccess {
        GpuFrameAccess {
            backend,
            handle: self.handle,
            cpu_ptr: self.cpu_ptr,
            size: self.size,
            width: request.width,
            height: request.height,
            row_bytes: request.row_bytes,
            pixel_format: request.pixel_format,
            #[cfg(feature = "wgpu")]
            wgpu_buffer: self.wgpu_buffer.clone(),
            #[cfg(feature = "wgpu")]
            wgpu_texture: self.wgpu_texture.clone(),
        }
    }
}

impl Drop for AllocatedGpuBuffer {
    fn drop(&mut self) {
        if let Some(drop) = self.drop.take() {
            drop();
        }
    }
}

/// Snapshot attached to a captured or scheduled frame.
#[derive(Clone, Debug)]
pub struct GpuFrameAccess {
    pub backend: GpuBackend,
    pub handle: NativeGpuHandle,
    pub cpu_ptr: *const u8,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub row_bytes: u32,
    pub pixel_format: PixelFormat,
    #[cfg(feature = "wgpu")]
    pub wgpu_buffer: Option<wgpu::Buffer>,
    #[cfg(feature = "wgpu")]
    pub wgpu_texture: Option<wgpu::Texture>,
}

unsafe impl Send for GpuFrameAccess {}

impl GpuFrameAccess {
    /// Packed UYVY as `Rgba8Unorm` with width/2, matching eiviz ingest.
    pub fn packed_uyvy_extent(&self) -> (u32, u32) {
        ((self.width / 2).max(1), self.height.max(1))
    }
}

/// Creates CPU-mapped buffers that DeckLink and the GPU can share.
pub trait GpuBufferFactory: Send + Sync {
    fn backend(&self) -> GpuBackend;
    fn allocate(&self, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer>;
}

/// System-memory fallback. Useful in tests and when HAL import is unavailable.
#[derive(Clone, Debug, Default)]
pub struct CpuSharedFactory;

impl GpuBufferFactory for CpuSharedFactory {
    fn backend(&self) -> GpuBackend {
        GpuBackend::Cpu
    }

    fn allocate(&self, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
        let host = pinned::PinnedHost::allocate(request.byte_size as usize)?;
        let (cpu_ptr, size, drop) = host.into_drop();
        Ok(AllocatedGpuBuffer {
            cpu_ptr,
            size,
            handle: NativeGpuHandle::None,
            #[cfg(feature = "wgpu")]
            wgpu_buffer: None,
            #[cfg(feature = "wgpu")]
            wgpu_texture: None,
            _cpu: None,
            drop: Some(drop),
        })
    }
}

const BUFFER_CACHE_LIMIT: usize = 8;

pub(crate) struct GpuRegistry {
    factory: Arc<dyn GpuBufferFactory>,
    live: Mutex<HashMap<usize, LiveGpu>>,
    cache: Mutex<Vec<AllocatedGpuBuffer>>,
}

struct LiveGpu {
    access: GpuFrameAccess,
    _keep: AllocatedGpuBuffer,
}

impl GpuRegistry {
    pub fn new(factory: Arc<dyn GpuBufferFactory>) -> Arc<Self> {
        Arc::new(Self {
            factory,
            live: Mutex::new(HashMap::new()),
            cache: Mutex::new(Vec::with_capacity(BUFFER_CACHE_LIMIT)),
        })
    }

    #[allow(dead_code)]
    pub fn backend(&self) -> GpuBackend {
        self.factory.backend()
    }

    pub fn allocate(&self, request: GpuBufferRequest) -> Result<(*mut u8, usize)> {
        let cached = {
            let mut cache = self.cache.lock().expect("gpu cache");
            cache
                .iter()
                .position(|slot| slot.size == request.byte_size as usize)
                .map(|index| cache.swap_remove(index))
        };
        let allocated = match cached {
            Some(allocated) => allocated,
            None => self.factory.allocate(request)?,
        };
        let cpu_ptr = allocated.cpu_ptr;
        let size = allocated.size;
        if cpu_ptr.is_null() {
            return Err(Error::new(
                ErrorKind::Sdk,
                "gpu_alloc",
                "allocator returned a null pointer",
            ));
        }
        let access = GpuFrameAccess {
            backend: self.factory.backend(),
            handle: allocated.handle,
            cpu_ptr,
            size,
            width: request.width,
            height: request.height,
            row_bytes: request.row_bytes,
            pixel_format: request.pixel_format,
            #[cfg(feature = "wgpu")]
            wgpu_buffer: allocated.wgpu_buffer.clone(),
            #[cfg(feature = "wgpu")]
            wgpu_texture: allocated.wgpu_texture.clone(),
        };
        self.live.lock().expect("gpu registry").insert(
            cpu_ptr as usize,
            LiveGpu {
                access,
                _keep: allocated,
            },
        );
        Ok((cpu_ptr, size))
    }

    pub fn release(&self, cpu: *mut u8) {
        let Some(live) = self.live.lock().expect("gpu registry").remove(&(cpu as usize)) else {
            return;
        };
        let mut cache = self.cache.lock().expect("gpu cache");
        if cache.len() < BUFFER_CACHE_LIMIT {
            cache.push(live._keep);
        }
    }

    pub fn lookup(&self, cpu: *const u8) -> Option<GpuFrameAccess> {
        self.live
            .lock()
            .expect("gpu registry")
            .get(&(cpu as usize))
            .map(|live| live.access.clone())
    }
}

pub(crate) unsafe extern "C" fn alloc_video_buffer(
    ctx: *mut c_void,
    size: u32,
    width: u32,
    height: u32,
    row_bytes: u32,
    pixel_format: u32,
    out: *mut decklink_sys::ExternalBuffer,
) -> i32 {
    let Some(registry) = (unsafe { (ctx as *const GpuRegistry).as_ref() }) else {
        return decklink_sys::E_INVALIDARG;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return decklink_sys::E_INVALIDARG;
    };
    match registry.allocate(GpuBufferRequest {
        width,
        height,
        row_bytes,
        byte_size: size,
        pixel_format: PixelFormat(pixel_format),
    }) {
        Ok((cpu, actual)) => {
            out.cpu = cpu.cast();
            out.size = actual as u64;
            decklink_sys::S_OK
        }
        Err(_) => decklink_sys::E_OUTOFMEMORY,
    }
}

pub(crate) unsafe extern "C" fn free_video_buffer(ctx: *mut c_void, cpu: *mut c_void) {
    if let Some(registry) = unsafe { (ctx as *const GpuRegistry).as_ref() } {
        registry.release(cpu.cast());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_factory_round_trip() {
        let registry = GpuRegistry::new(Arc::new(CpuSharedFactory));
        let (ptr, size) = registry
            .allocate(GpuBufferRequest {
                width: 8,
                height: 2,
                row_bytes: 16,
                byte_size: 32,
                pixel_format: PixelFormat::YUV_8BIT,
            })
            .unwrap();
        assert!(!ptr.is_null());
        assert_eq!(size, 32);
        assert_eq!(ptr as usize % pinned::PIN_ALIGN, 0);
        assert!(registry.lookup(ptr).is_some());
        registry.release(ptr);
        assert!(registry.lookup(ptr).is_none());
        let (again, _) = registry
            .allocate(GpuBufferRequest {
                width: 8,
                height: 2,
                row_bytes: 16,
                byte_size: 32,
                pixel_format: PixelFormat::YUV_8BIT,
            })
            .unwrap();
        assert_eq!(again, ptr);
        registry.release(again);
    }
}
