//! Page-aligned host memory that DeckLink can lock for DMA.
//!
//! Windows uses `VirtualAlloc` (SDK DX11 samples). Unix uses `posix_memalign(4096)`
//! (SDK OpenGL samples). Both fall back to a 4 KB `std::alloc` layout.

#![allow(clippy::undocumented_unsafe_blocks)]

use std::alloc::{Layout, alloc_zeroed, dealloc};

use crate::error::{Error, ErrorKind, Result};

/// Alignment used by Blackmagic's OpenGL / DX11 samples.
pub(crate) const PIN_ALIGN: usize = 4096;

pub(crate) struct PinnedHost {
    pub ptr: *mut u8,
    pub size: usize,
    drop: Option<Box<dyn FnOnce() + Send>>,
}

unsafe impl Send for PinnedHost {}

impl Drop for PinnedHost {
    fn drop(&mut self) {
        if let Some(drop) = self.drop.take() {
            drop();
        }
    }
}

impl PinnedHost {
    pub(crate) fn allocate(size: usize) -> Result<Self> {
        let size = size.max(1);
        #[cfg(windows)]
        if let Ok(host) = allocate_virtual(size) {
            return Ok(host);
        }
        #[cfg(unix)]
        if let Ok(host) = allocate_posix(size) {
            return Ok(host);
        }
        allocate_aligned(size)
    }

    pub(crate) fn into_drop(mut self) -> (*mut u8, usize, Box<dyn FnOnce() + Send>) {
        let ptr = self.ptr;
        let size = self.size;
        let drop = self.drop.take().expect("pinned host drop");
        std::mem::forget(self);
        (ptr, size, drop)
    }
}

fn allocate_aligned(size: usize) -> Result<PinnedHost> {
    let layout = Layout::from_size_align(size, PIN_ALIGN).map_err(|err| {
        Error::new(
            ErrorKind::Unsupported,
            "pinned_alloc",
            format!("invalid pinned layout: {err}"),
        )
    })?;
    let ptr = unsafe { alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err(Error::new(
            ErrorKind::Sdk,
            "pinned_alloc",
            "aligned allocation returned null",
        ));
    }
    let addr = ptr as usize;
    Ok(PinnedHost {
        ptr,
        size,
        drop: Some(Box::new(move || unsafe {
            dealloc(addr as *mut u8, layout);
        })),
    })
}

#[cfg(unix)]
fn allocate_posix(size: usize) -> Result<PinnedHost> {
    let mut ptr = std::ptr::null_mut();
    let err = unsafe { posix_memalign(&mut ptr, PIN_ALIGN, size) };
    if err != 0 || ptr.is_null() {
        return Err(Error::new(
            ErrorKind::Sdk,
            "posix_memalign",
            format!("posix_memalign failed ({err})"),
        ));
    }
    // posix_memalign does not zero memory; DeckLink / tests expect a defined buffer.
    unsafe { std::ptr::write_bytes(ptr, 0, size) };
    let addr = ptr as usize;
    Ok(PinnedHost {
        ptr: ptr.cast(),
        size,
        drop: Some(Box::new(move || unsafe {
            free(addr as *mut _);
        })),
    })
}

#[cfg(unix)]
unsafe extern "C" {
    fn posix_memalign(memptr: *mut *mut std::ffi::c_void, alignment: usize, size: usize) -> i32;
    fn free(ptr: *mut std::ffi::c_void);
}

#[cfg(windows)]
fn allocate_virtual(size: usize) -> Result<PinnedHost> {
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };

    let ptr = unsafe { VirtualAlloc(None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    if ptr.is_null() {
        return Err(Error::new(
            ErrorKind::Sdk,
            "virtual_alloc",
            "VirtualAlloc returned null",
        ));
    }
    let addr = ptr as usize;
    Ok(PinnedHost {
        ptr: ptr.cast::<u8>(),
        size,
        drop: Some(Box::new(move || unsafe {
            let _ = VirtualFree(addr as *mut _, 0, MEM_RELEASE);
        })),
    })
}
