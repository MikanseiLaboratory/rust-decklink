#![allow(clippy::undocumented_unsafe_blocks)]

use decklink_sys::{api_version, rdl_hardware_available, rdl_initialize, rdl_uninitialize};

#[test]
fn initialize_and_availability_match_the_build_mode() {
    let available = unsafe { rdl_hardware_available() };
    let hr = unsafe { rdl_initialize() };
    if decklink_sys::HARDWARE_COMPILED {
        // The real shim always initializes. Availability depends on the
        // Desktop Video driver: present on a capture machine, absent on CI
        // runners and dev machines without a card.
        assert!(hr >= 0);
        if available == 0 {
            // No driver: the SDK dispatch returns a null instance and the
            // shim answers with the compile-time fallback string.
            assert_eq!(api_version().unwrap(), "16.0");
        } else {
            assert!(!api_version().unwrap().is_empty());
        }
    } else {
        // The stub cannot do anything.
        assert_eq!(available, 0);
        assert!(hr < 0);
        assert!(api_version().is_ok());
    }
    unsafe { rdl_uninitialize() };
}

#[test]
fn hresult_constants_keep_raw_values() {
    assert_eq!(decklink_sys::S_OK, 0);
    assert_eq!(decklink_sys::S_FALSE, 1);
    assert_eq!(decklink_sys::E_FAIL, 0x8000_0008u32 as i32);
}
