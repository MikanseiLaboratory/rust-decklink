//! Shared buffers for DeckLink capture / playout.
//!
//! Callers always see a CPU pointer. DeckLink DMA-locks that pointer.
//! Windows uses `VirtualAlloc`, Unix uses `posix_memalign(4096)`, macOS Metal
//! uses `MTLStorageModeShared`. wgpu may wrap the same pages (Metal today);
//! GPU BAR / `GPU_UPLOAD` is never given to DeckLink.

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

impl GpuBufferRequest {
    /// Packed frame sized as `row_bytes * height`.
    pub fn packed(width: u32, height: u32, row_bytes: u32, pixel_format: PixelFormat) -> Self {
        Self {
            width,
            height,
            row_bytes,
            byte_size: row_bytes.saturating_mul(height.max(1)),
            pixel_format,
        }
    }
}

/// Allocation returned to the DeckLink allocator callback.
pub struct AllocatedGpuBuffer {
    pub backend: GpuBackend,
    pub cpu_ptr: *mut u8,
    pub size: usize,
    pub handle: NativeGpuHandle,
    request: GpuBufferRequest,
    #[cfg(feature = "wgpu")]
    pub wgpu_buffer: Option<wgpu::Buffer>,
    #[cfg(feature = "wgpu")]
    pub wgpu_texture: Option<wgpu::Texture>,
    drop: Option<Box<dyn FnOnce() + Send>>,
}

unsafe impl Send for AllocatedGpuBuffer {}

impl AllocatedGpuBuffer {
    pub(crate) fn pinned(request: GpuBufferRequest) -> Result<Self> {
        let host = pinned::PinnedHost::allocate(request.byte_size as usize)?;
        let (cpu_ptr, size, drop) = host.into_drop();
        Ok(Self {
            backend: GpuBackend::Cpu,
            cpu_ptr,
            size,
            handle: NativeGpuHandle::None,
            request,
            #[cfg(feature = "wgpu")]
            wgpu_buffer: None,
            #[cfg(feature = "wgpu")]
            wgpu_texture: None,
            drop: Some(drop),
        })
    }

    /// Snapshot DeckLink / wgpu can both use. Dimensions come from allocation.
    pub fn access(&self) -> GpuFrameAccess {
        GpuFrameAccess {
            backend: self.backend,
            handle: self.handle,
            cpu_ptr: self.cpu_ptr,
            size: self.size,
            width: self.request.width,
            height: self.request.height,
            row_bytes: self.request.row_bytes,
            pixel_format: self.request.pixel_format,
            #[cfg(feature = "wgpu")]
            wgpu_buffer: self.wgpu_buffer.clone(),
            #[cfg(feature = "wgpu")]
            wgpu_texture: self.wgpu_texture.clone(),
        }
    }

    /// Writable view of the DeckLink-visible bytes.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        if self.cpu_ptr.is_null() {
            &mut []
        } else {
            // SAFETY: `cpu_ptr` is owned by this allocation for `size` bytes.
            unsafe { std::slice::from_raw_parts_mut(self.cpu_ptr, self.size) }
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
        AllocatedGpuBuffer::pinned(request)
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
        let access = allocated.access();
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

    #[test]
    fn packed_request_uses_row_times_height() {
        let request = GpuBufferRequest::packed(8, 2, 16, PixelFormat::YUV_8BIT);
        assert_eq!(request.byte_size, 32);
    }
}
