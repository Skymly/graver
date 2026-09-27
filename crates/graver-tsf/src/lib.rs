// link.exe prints a status line when it writes the import library.
#![allow(linker_messages)]

//! In-process Text Services Framework front end.
//!
//! Windows loads this DLL into every application that uses Graver. Keep it
//! thin: no dictionary, no UI runtime, and no async runtime. The composition
//! engine stays in graver-service. `DllGetClassObject` can create the text
//! service. Registration stays unimplemented, so regsvr32 fails and Graver is
//! not advertised to Windows.
//!
//! A registered text service also needs an i686 build with the same CLSID.
//! This scaffold builds the host architecture only.
//!
//! This crate does not define DllMain. Do not connect the pipe, create a thread,
//! or initialize COM from DllMain.

mod client;
mod decision;
mod factory;
mod protocol;
mod service;

pub use client::{ClientError, IO_TIMEOUT_MS, Session};
pub use decision::{KeyDecision, KeyModifiers, PreviewKey, decide_key, preview_eaten};
pub use protocol::{
    ClientRequest, ClientResponse, CompositionUpdate, KeyKind, KeyRequest, PIPE_NAME,
    PROTOCOL_VERSION, ProtocolError,
};

use core::ffi::c_void;

use windows::{
    Win32::{
        Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_INVALIDARG, E_NOTIMPL, E_POINTER, S_OK},
        UI::TextServices::ITfTextInputProcessor,
    },
    core::{GUID, HRESULT, Interface},
};

use factory::class_object;

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
/// COM `DllGetClassObject` contract. On failure the slot is set to null.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: a null slot cannot be written. COM callers pass a valid slot.
    if ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = core::ptr::null_mut() };
    if rclsid.is_null() || unsafe { *rclsid } != TEXT_SERVICE_CLSID {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    if riid.is_null() {
        return E_INVALIDARG;
    }
    class_object(riid, ppv)
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
    use windows::Win32::{System::Com::IClassFactory, UI::TextServices::ITfKeyEventSink};

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
    #[test]
    fn class_factory_creates_the_text_service() {
        let mut slot = core::ptr::dangling_mut::<c_void>();
        let hr = unsafe { DllGetClassObject(&TEXT_SERVICE_CLSID, &IClassFactory::IID, &mut slot) };
        assert_eq!(hr, S_OK);
        assert!(!slot.is_null());
        let factory = unsafe { IClassFactory::from_raw(slot) };
        let service: ITfTextInputProcessor = unsafe { factory.CreateInstance(None) }.unwrap();
        let _sink: ITfKeyEventSink = service.cast().unwrap();
    }

    #[test]
    fn wrong_clsid_or_iid_fails_and_clears_the_slot() {
        let mut slot = core::ptr::dangling_mut::<c_void>();
        let hr = unsafe { DllGetClassObject(&PROFILE_GUID, &IClassFactory::IID, &mut slot) };
        assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
        assert!(slot.is_null());

        slot = core::ptr::dangling_mut::<c_void>();
        let hr = unsafe { DllGetClassObject(&TEXT_SERVICE_CLSID, &PROFILE_GUID, &mut slot) };
        assert_eq!(hr, windows::Win32::Foundation::E_NOINTERFACE);
        assert!(slot.is_null());

        slot = core::ptr::dangling_mut::<c_void>();
        let factory_slot = {
            let hr =
                unsafe { DllGetClassObject(&TEXT_SERVICE_CLSID, &IClassFactory::IID, &mut slot) };
            assert_eq!(hr, S_OK);
            slot
        };
        let factory = unsafe { IClassFactory::from_raw(factory_slot) };
        let mut created = core::ptr::dangling_mut::<c_void>();
        let hr = unsafe {
            (Interface::vtable(&factory).CreateInstance)(
                Interface::as_raw(&factory),
                core::ptr::null_mut(),
                &PROFILE_GUID,
                &mut created,
            )
        };
        assert!(hr.is_err());
        assert!(created.is_null());
    }
}
