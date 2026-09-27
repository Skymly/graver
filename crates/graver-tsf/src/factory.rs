//! Class factory for the unregistered text service.
//!
//! `DllGetClassObject` can create the in-process object. Registration stays
//! unimplemented, so `regsvr32` fails and Graver is not advertised to Windows.

use core::ffi::c_void;

use windows::{
    Win32::{
        Foundation::{CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::ITfTextInputProcessor,
    },
    core::{GUID, HRESULT, IUnknown, Interface, Ref, implement},
};

use crate::service::TextService;

#[implement(IClassFactory)]
pub(crate) struct ClassFactory;

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        // SAFETY: `ppvobject` is either null or a caller-owned pointer slot.
        // The slot is cleared before any failure return so a rejected IID cannot
        // leak a previous pointer.
        unsafe {
            if ppvobject.is_null() {
                return Err(E_POINTER.into());
            }
            *ppvobject = core::ptr::null_mut();
            if riid.is_null() {
                return Err(E_INVALIDARG.into());
            }
            if !punkouter.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            let service: ITfTextInputProcessor = TextService::production().into();
            let hr = service.query(riid, ppvobject);
            if hr.is_err() {
                *ppvobject = core::ptr::null_mut();
                return Err(hr.into());
            }
            Ok(())
        }
    }

    fn LockServer(&self, _flock: windows_core::BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

pub(crate) fn class_object(riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    let factory: IClassFactory = ClassFactory.into();
    // SAFETY: `ppv` is non-null. `riid` is non-null. Both are checked by the caller.
    let hr = unsafe { factory.query(riid, ppv) };
    if hr.is_err() {
        // SAFETY: same caller-owned slot. Clear it if QueryInterface failed after writing.
        unsafe { *ppv = core::ptr::null_mut() };
    }
    hr
}
