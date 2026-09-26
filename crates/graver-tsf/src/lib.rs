// link.exe prints a status line when it writes the import library.
#![allow(linker_messages)]

//! In-process Text Services Framework front end.
//!
//! Windows loads this DLL into every application that uses Graver. Keep it
//! thin: no dictionary, no UI runtime, and no async runtime. The composition
//! engine stays in graver-service. Registration is intentionally unimplemented,
//! so regsvr32 fails instead of advertising a broken service.
//!
//! A registered text service also needs an i686 build with the same CLSID.
//! This scaffold builds the host architecture only.
//!
//! This crate does not define DllMain. Do not connect the pipe, create a thread,
//! or initialize COM from DllMain.

mod client;
mod protocol;

pub use client::{ClientError, IO_TIMEOUT_MS, Session};
pub use protocol::{
    ClientRequest, ClientResponse, CompositionUpdate, KeyKind, KeyRequest, PIPE_NAME,
    PROTOCOL_VERSION, ProtocolError,
};

use windows::{
    Win32::{
        Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_NOTIMPL, S_OK},
        UI::TextServices::ITfTextInputProcessor,
    },
    core::{GUID, HRESULT, Interface},
};

/// {91a1da98-cdfb-4d2e-8b83-4a2c18e65adf}
pub const TEXT_SERVICE_CLSID: GUID = GUID::from_u128(0x91a1da98_cdfb_4d2e_8b83_4a2c18e65adf);

/// {cc7adea0-2390-4dd0-8193-63fa8f04e057}
pub const PROFILE_GUID: GUID = GUID::from_u128(0xcc7adea0_2390_4dd0_8193_63fa8f04e057);

pub const DISPLAY_NAME: &str = "Graver";

/// Simplified Chinese. Stored for the future language profile; not registered.
pub const LANGID_ZH_CN: u16 = 0x0804;

pub fn text_input_processor_iid() -> GUID {
    ITfTextInputProcessor::IID
}

/// # Safety
///
/// `ppv` must be null or point to a caller-owned pointer slot. This is the
/// COM `DllGetClassObject` contract. The text service is not implemented, so
/// the slot is cleared and the call fails.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    _rclsid: *const GUID,
    _riid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> HRESULT {
    if !ppv.is_null() {
        unsafe {
            *ppv = core::ptr::null_mut();
        }
    }
    CLASS_E_CLASSNOTAVAILABLE
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    E_NOTIMPL
}

#[unsafe(no_mangle)]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    E_NOTIMPL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_stay_distinct() {
        assert_ne!(TEXT_SERVICE_CLSID, GUID::from_u128(0));
        assert_ne!(PROFILE_GUID, TEXT_SERVICE_CLSID);
        assert_ne!(text_input_processor_iid(), GUID::from_u128(0));
        assert_eq!(DISPLAY_NAME, "Graver");
        assert_eq!(LANGID_ZH_CN, 0x0804);
    }

    #[test]
    fn registration_is_not_implemented() {
        let mut slot = core::ptr::dangling_mut::<core::ffi::c_void>();
        assert_eq!(
            unsafe { DllGetClassObject(core::ptr::null(), core::ptr::null(), &mut slot) },
            CLASS_E_CLASSNOTAVAILABLE
        );
        assert!(slot.is_null());
        assert_eq!(DllCanUnloadNow(), S_OK);
        assert_eq!(DllRegisterServer(), E_NOTIMPL);
        assert_eq!(DllUnregisterServer(), E_NOTIMPL);
    }
}
