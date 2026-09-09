#![allow(clippy::undocumented_unsafe_blocks)]

use decklink_sys::{api_version, rdl_hardware_available, rdl_initialize, rdl_uninitialize};

#[test]
fn stub_or_hardware_initialize_is_safe() {
    let available = unsafe { rdl_hardware_available() };
    let hr = unsafe { rdl_initialize() };
    if available == 0 {
        assert!(hr < 0);
        assert!(api_version().is_ok());
    } else {
        assert!(hr >= 0);
        let version = api_version().unwrap();
        assert!(!version.is_empty());
    }
    unsafe { rdl_uninitialize() };
}

#[test]
fn hresult_constants_keep_raw_values() {
    assert_eq!(decklink_sys::S_OK, 0);
    assert_eq!(decklink_sys::S_FALSE, 1);
    assert_eq!(decklink_sys::E_FAIL, 0x8000_0008u32 as i32);
}
