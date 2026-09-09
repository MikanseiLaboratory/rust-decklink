//! wgpu 30 helpers that match eiviz mixer ingest (DX12 GPU upload, Metal shared).

#![allow(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "macos")]
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::error::{Error, ErrorKind, Result};
use crate::gpu::{
    AllocatedGpuBuffer, CpuSharedFactory, GpuBackend, GpuBufferFactory, GpuBufferRequest, NativeGpuHandle,
};

/// Shared-memory factory backed by the same wgpu device eiviz already owns.
#[derive(Clone)]
pub struct WgpuSharedFactory {
    device: wgpu::Device,
    backend: wgpu::Backend,
}

impl WgpuSharedFactory {
    pub fn new(device: wgpu::Device, backend: wgpu::Backend) -> Self {
        Self { device, backend }
    }
}

impl GpuBufferFactory for WgpuSharedFactory {
    fn backend(&self) -> GpuBackend {
        match self.backend {
            wgpu::Backend::Dx12 => GpuBackend::D3D12,
            wgpu::Backend::Metal => GpuBackend::Metal,
            wgpu::Backend::Vulkan => GpuBackend::Vulkan,
            _ => GpuBackend::Cpu,
        }
    }

    fn allocate(&self, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
        #[cfg(windows)]
        if self.backend == wgpu::Backend::Dx12 {
            if let Ok(buffer) = allocate_dx12(&self.device, request) {
                return Ok(buffer);
            }
        }
        #[cfg(target_os = "macos")]
        if self.backend == wgpu::Backend::Metal {
            if let Ok(buffer) = allocate_metal(&self.device, request) {
                return Ok(buffer);
            }
        }
        #[cfg(any(windows, target_os = "linux"))]
        if self.backend == wgpu::Backend::Vulkan {
            if let Ok(buffer) = allocate_vulkan(&self.device, request) {
                return Ok(buffer);
            }
        }
        CpuSharedFactory.allocate(request)
    }
}

#[cfg(windows)]
fn allocate_dx12(device: &wgpu::Device, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
    use windows::Win32::Graphics::Direct3D12::{
        D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
        D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

    let d3d = unsafe {
        device
            .as_hal::<wgpu::hal::api::Dx12>()
            .map(|hal| hal.raw_device().clone())
    }
    .ok_or_else(|| Error::new(ErrorKind::Unsupported, "wgpu", "device is not D3D12"))?;

    let bytes = request.byte_size.max(256) as u64;
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: Default::default(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: Default::default(),
    };

    let resource = create_upload_buffer(&d3d, &desc).or_else(|_| {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        };
        create_committed(&d3d, &heap, &desc)
    })?;

    let mut cpu = std::ptr::null_mut();
    unsafe {
        resource
            .Map(0, None, Some(&mut cpu))
            .map_err(|err| Error::new(ErrorKind::Sdk, "d3d12_map", err.to_string()))?;
    }
    let cpu_ptr = cpu.cast::<u8>();
    let raw = windows::core::Interface::as_raw(&resource);
    let handle = NativeGpuHandle::D3D12Resource(NonNull::new(raw).expect("d3d12 resource"));
    let hal = unsafe { wgpu::hal::dx12::Device::buffer_from_raw(resource.clone(), bytes) };
    let wgpu_buffer = unsafe {
        device.create_buffer_from_hal::<wgpu::hal::api::Dx12>(
            hal,
            &wgpu::BufferDescriptor {
                label: Some("decklink gpu upload"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    };
    let keep = resource;
    Ok(AllocatedGpuBuffer {
        cpu_ptr,
        size: request.byte_size as usize,
        handle,
        wgpu_buffer: Some(wgpu_buffer),
        wgpu_texture: None,
        _cpu: None,
        drop: Some(Box::new(move || unsafe {
            keep.Unmap(0, None);
        })),
    })
}

#[cfg(windows)]
fn create_upload_buffer(
    d3d: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    desc: &windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC,
) -> Result<windows::Win32::Graphics::Direct3D12::ID3D12Resource> {
    use windows::Win32::Graphics::Direct3D12::{D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_GPU_UPLOAD};
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_GPU_UPLOAD,
        ..Default::default()
    };
    create_committed(d3d, &heap, desc)
}

#[cfg(windows)]
fn create_committed(
    d3d: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    heap: &windows::Win32::Graphics::Direct3D12::D3D12_HEAP_PROPERTIES,
    desc: &windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC,
) -> Result<windows::Win32::Graphics::Direct3D12::ID3D12Resource> {
    use windows::Win32::Graphics::Direct3D12::{
        ID3D12Resource, D3D12_HEAP_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ,
    };
    let mut resource = None;
    unsafe {
        d3d.CreateCommittedResource::<ID3D12Resource>(
            heap,
            D3D12_HEAP_FLAG_NONE,
            desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut resource,
        )
        .map_err(|err| Error::new(ErrorKind::Sdk, "d3d12_buffer", err.to_string()))?;
    }
    resource.ok_or_else(|| Error::new(ErrorKind::Sdk, "d3d12_buffer", "CreateCommittedResource returned null"))
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
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    };
    Ok(AllocatedGpuBuffer {
        cpu_ptr,
        size,
        handle,
        wgpu_buffer: Some(wgpu_buffer),
        wgpu_texture: None,
        _cpu: None,
        drop: Some(Box::new(move || {
            drop(raw_buf);
        })),
    })
}

#[cfg(any(windows, target_os = "linux"))]
fn allocate_vulkan(device: &wgpu::Device, request: GpuBufferRequest) -> Result<AllocatedGpuBuffer> {
    use ash::vk;
    use ash::vk::Handle;

    let (instance, vk_device, physical) = unsafe {
        device.as_hal::<wgpu::hal::api::Vulkan>().map(|hal| {
            (
                hal.shared_instance().raw_instance().clone(),
                hal.raw_device().clone(),
                hal.raw_physical_device(),
            )
        })
    }
    .ok_or_else(|| Error::new(ErrorKind::Unsupported, "wgpu", "device is not Vulkan"))?;

    let bytes = u64::from(request.byte_size.max(1));
    let info = vk::BufferCreateInfo::default()
        .size(bytes)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe {
        vk_device
            .create_buffer(&info, None)
            .map_err(|err| Error::new(ErrorKind::Sdk, "vk_buffer", err.to_string()))?
    };
    let req = unsafe { vk_device.get_buffer_memory_requirements(buffer) };
    let props = unsafe { instance.get_physical_device_memory_properties(physical) };
    let Some(type_index) = find_host_visible_memory(&props, req.memory_type_bits) else {
        unsafe { vk_device.destroy_buffer(buffer, None) };
        return Err(Error::new(
            ErrorKind::Unsupported,
            "vk_memory",
            "no HOST_VISIBLE memory type for a DeckLink buffer",
        ));
    };
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(type_index);
    let memory = unsafe {
        match vk_device.allocate_memory(&alloc, None) {
            Ok(memory) => memory,
            Err(err) => {
                vk_device.destroy_buffer(buffer, None);
                return Err(Error::new(ErrorKind::Sdk, "vk_memory", err.to_string()));
            }
        }
    };
    if let Err(err) = unsafe { vk_device.bind_buffer_memory(buffer, memory, 0) } {
        unsafe {
            vk_device.free_memory(memory, None);
            vk_device.destroy_buffer(buffer, None);
        }
        return Err(Error::new(ErrorKind::Sdk, "vk_bind", err.to_string()));
    }
    let ptr = unsafe {
        match vk_device.map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty()) {
            Ok(ptr) => ptr,
            Err(err) => {
                vk_device.free_memory(memory, None);
                vk_device.destroy_buffer(buffer, None);
                return Err(Error::new(ErrorKind::Sdk, "vk_map", err.to_string()));
            }
        }
    };
    let handle = NativeGpuHandle::VulkanBuffer(buffer.as_raw());
    let hal = unsafe { wgpu::hal::vulkan::Buffer::from_raw_managed(buffer, memory, 0, req.size) };
    let wgpu_buffer = unsafe {
        device.create_buffer_from_hal::<wgpu::hal::api::Vulkan>(
            hal,
            &wgpu::BufferDescriptor {
                label: Some("decklink vulkan host-visible"),
                size: req.size,
                usage: wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    };
    let unmap = vk_device.clone();
    Ok(AllocatedGpuBuffer {
        cpu_ptr: ptr.cast(),
        size: request.byte_size as usize,
        handle,
        wgpu_buffer: Some(wgpu_buffer),
        wgpu_texture: None,
        _cpu: None,
        drop: Some(Box::new(move || unsafe {
            unmap.unmap_memory(memory);
        })),
    })
}

#[cfg(any(windows, target_os = "linux"))]
fn find_host_visible_memory(props: &ash::vk::PhysicalDeviceMemoryProperties, type_bits: u32) -> Option<u32> {
    use ash::vk::MemoryPropertyFlags as Flags;
    let wanted = [
        Flags::DEVICE_LOCAL | Flags::HOST_VISIBLE | Flags::HOST_COHERENT,
        Flags::DEVICE_LOCAL | Flags::HOST_VISIBLE,
        Flags::HOST_VISIBLE | Flags::HOST_COHERENT,
        Flags::HOST_VISIBLE,
    ];
    for flags in wanted {
        for (index, mem) in props.memory_types_as_slice().iter().enumerate() {
            if type_bits & (1 << index) == 0 {
                continue;
            }
            if mem.property_flags.contains(flags) {
                return Some(index as u32);
            }
        }
    }
    None
}
