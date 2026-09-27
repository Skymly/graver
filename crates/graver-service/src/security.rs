//! Current-user security descriptor for the production named pipe.
//!
//! A null descriptor uses the default DACL, which grants Everyone read access.
//! This module grants only the calling user. AppContainer hosts may be unable
//! to connect later; do not add ALL APPLICATION PACKAGES or any other broad SID.
//! The text service returns the key when it cannot connect.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::{
    Win32::{
        Foundation::HANDLE,
        Security::{
            ACL, ACL_REVISION, AddAccessAllowedAce, CopySid, GetLengthSid, GetTokenInformation,
            InitializeAcl, InitializeSecurityDescriptor, IsValidSecurityDescriptor,
            PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
            SECURITY_DESCRIPTOR, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
            SetSecurityDescriptorOwner, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
    core::Error,
};

use crate::ServiceError;

/// `SECURITY_DESCRIPTOR_REVISION` from winnt.h. Kept local so this crate does
/// not need the SystemServices feature.
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

/// Owns the absolute security descriptor passed to `CreateNamedPipeW`.
///
/// The descriptor points at the ACL and owner SID stored in this value. Those
/// buffers are heap allocations, so moving this value does not invalidate them.
/// [`Self::attributes`] borrows the descriptor itself and must not outlive the
/// `CreateNamedPipeW` call.
pub struct CurrentUserPipeSecurity {
    sd: SECURITY_DESCRIPTOR,
    acl: Vec<u32>,
    owner: Vec<u64>,
}

impl CurrentUserPipeSecurity {
    pub fn for_current_user() -> Result<Self, ServiceError> {
        let owner = current_user_sid()?;
        let acl = allow_only(PSID(owner.as_ptr() as *mut _))?;
        let mut sd = SECURITY_DESCRIPTOR::default();
        let sd_ptr = PSECURITY_DESCRIPTOR(&mut sd as *mut SECURITY_DESCRIPTOR as *mut _);
        // SAFETY: `sd` is a writable absolute descriptor, `acl` and `owner` are
        // aligned heap buffers that outlive `sd_ptr` until this function returns
        // and the caller keeps them in the returned struct.
        unsafe {
            InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION)
                .map_err(windows_error)?;
            SetSecurityDescriptorDacl(sd_ptr, true, Some(acl.as_ptr() as *const ACL), false)
                .map_err(windows_error)?;
            SetSecurityDescriptorOwner(sd_ptr, Some(PSID(owner.as_ptr() as *mut _)), false)
                .map_err(windows_error)?;
            SetSecurityDescriptorControl(sd_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
                .map_err(windows_error)?;
            if !IsValidSecurityDescriptor(sd_ptr).as_bool() {
                return Err(ServiceError::Windows(
                    "current-user pipe security descriptor is invalid".into(),
                ));
            }
        }
        Ok(Self { sd, acl, owner })
    }

    pub fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        // Touch the buffers so they stay alive with the pointers stored in `sd`.
        let _ = (self.acl.len(), self.owner.len());
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &mut self.sd as *mut SECURITY_DESCRIPTOR as *mut _,
            bInheritHandle: false.into(),
        }
    }
}

fn allow_only(sid: PSID) -> Result<Vec<u32>, ServiceError> {
    let sid_len = unsafe { GetLengthSid(sid) } as usize;
    if sid_len == 0 {
        return Err(ServiceError::Windows("current user SID is empty".into()));
    }
    let acl_size = size_of::<ACL>() + size_of::<windows::Win32::Security::ACCESS_ALLOWED_ACE>()
        - size_of::<u32>()
        + sid_len;
    let acl_size = acl_size.div_ceil(4) * 4;
    let mut acl = vec![0u32; acl_size / 4];
    // SAFETY: `acl` is DWORD-aligned and large enough for one allow ACE. `sid`
    // remains valid for this call; AddAccessAllowedAce copies it into the ACL.
    unsafe {
        InitializeAcl(acl.as_mut_ptr() as *mut ACL, acl_size as u32, ACL_REVISION)
            .map_err(windows_error)?;
        AddAccessAllowedAce(
            acl.as_mut_ptr() as *mut ACL,
            ACL_REVISION,
            FILE_ALL_ACCESS.0,
            sid,
        )
        .map_err(windows_error)?;
    }
    Ok(acl)
}

fn current_user_sid() -> Result<Vec<u64>, ServiceError> {
    // SAFETY: GetCurrentProcess returns a pseudo-handle that must not be closed.
    // OpenProcessToken writes a real handle that OwnedHandle closes.
    let token = unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(windows_error)?;
        OwnedHandle::from_raw_handle(token.0)
    };
    let mut needed = 0u32;
    let _ = unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            None,
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(ServiceError::Windows(
            "TokenUser size was not returned".into(),
        ));
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` is pointer-aligned and at least `needed` bytes. The SID is
    // copied out before `buf` is dropped.
    let sid_len = unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        )
        .map_err(windows_error)?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid_len = GetLengthSid(user.User.Sid);
        if sid_len == 0 {
            return Err(ServiceError::Windows(
                "current user SID length is zero".into(),
            ));
        }
        sid_len
    };
    let mut owner = vec![0u64; (sid_len as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `owner` has room for `sid_len` bytes and is pointer-aligned.
    // `buf` is still alive, so the source SID is valid.
    unsafe {
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        CopySid(sid_len, PSID(owner.as_mut_ptr() as *mut _), user.User.Sid)
            .map_err(windows_error)?;
    }
    Ok(owner)
}

fn windows_error(err: Error) -> ServiceError {
    ServiceError::Windows(err.to_string())
}

#[cfg(test)]
pub(crate) mod inspect {
    use super::*;
    use std::mem::offset_of;
    use windows::{
        Win32::{
            Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree},
            Security::Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT},
            Security::{
                ACCESS_ALLOWED_ACE, ACE_HEADER, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
                EqualSid, GetAce, SECURITY_MAX_SID_SIZE, WELL_KNOWN_SID_TYPE, WinAnonymousSid,
                WinBuiltinAnyPackageSid, WinBuiltinUsersSid, WinWorldSid,
            },
        },
        core::PWSTR,
    };

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    pub(crate) struct PipeDacl {
        pub allow: Vec<Vec<u8>>,
        pub allow_strings: Vec<String>,
    }

    pub(crate) fn read_allow_entries(handle: HANDLE) -> Result<PipeDacl, ServiceError> {
        let mut dacl = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        // SAFETY: GetSecurityInfo allocates `sd` on success. The guard frees it
        // after the ACE SIDs have been copied out.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                Some(&mut sd),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(ServiceError::Windows(format!(
                "GetSecurityInfo failed: {}",
                status.0
            )));
        }
        let _guard = SdGuard(sd);
        if dacl.is_null() {
            return Err(ServiceError::Windows(
                "pipe DACL is null, which grants Everyone".into(),
            ));
        }
        let count = unsafe { (*dacl).AceCount };
        let mut allow = Vec::new();
        for index in 0..count {
            let mut ace = std::ptr::null_mut();
            unsafe { GetAce(dacl, u32::from(index), &mut ace).map_err(windows_error)? };
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                return Err(ServiceError::Windows(format!(
                    "pipe DACL contains non-allow ACE type {}",
                    header.AceType
                )));
            }
            let sid = unsafe { (ace as *const u8).add(offset_of!(ACCESS_ALLOWED_ACE, SidStart)) };
            let len = unsafe { GetLengthSid(PSID(sid as *mut _)) } as usize;
            if len == 0 {
                return Err(ServiceError::Windows("allow ACE SID is empty".into()));
            }
            let mut bytes = vec![0u8; len];
            unsafe { std::ptr::copy_nonoverlapping(sid, bytes.as_mut_ptr(), len) };
            allow.push(bytes);
        }
        let allow_strings = allow
            .iter()
            .map(|sid| sid_string(sid))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PipeDacl {
            allow,
            allow_strings,
        })
    }

    pub(crate) fn current_user_sid_bytes() -> Result<Vec<u8>, ServiceError> {
        let owner = current_user_sid()?;
        let len = unsafe { GetLengthSid(PSID(owner.as_ptr() as *mut _)) } as usize;
        let mut bytes = vec![0u8; len];
        unsafe {
            std::ptr::copy_nonoverlapping(owner.as_ptr() as *const u8, bytes.as_mut_ptr(), len);
        }
        Ok(bytes)
    }

    pub(crate) struct ForbiddenSid {
        pub label: &'static str,
        pub string_sid: &'static str,
        pub bytes: Vec<u8>,
    }

    pub(crate) fn forbidden_sids() -> Result<Vec<ForbiddenSid>, ServiceError> {
        Ok(vec![
            ForbiddenSid {
                label: "Everyone",
                string_sid: "S-1-1-0",
                bytes: well_known(WinWorldSid)?,
            },
            ForbiddenSid {
                label: "Anonymous",
                string_sid: "S-1-5-7",
                bytes: well_known(WinAnonymousSid)?,
            },
            ForbiddenSid {
                label: "ALL APPLICATION PACKAGES",
                string_sid: "S-1-15-2-1",
                bytes: well_known(WinBuiltinAnyPackageSid)?,
            },
            ForbiddenSid {
                label: "Users",
                string_sid: "S-1-5-32-545",
                bytes: well_known(WinBuiltinUsersSid)?,
            },
        ])
    }

    pub(crate) fn sid_equal(left: &[u8], right: &[u8]) -> bool {
        unsafe {
            EqualSid(
                PSID(left.as_ptr() as *mut _),
                PSID(right.as_ptr() as *mut _),
            )
            .is_ok()
        }
    }

    pub(crate) fn sid_string(sid: &[u8]) -> Result<String, ServiceError> {
        let mut wide = PWSTR::null();
        // SAFETY: `sid` points at a SID copied from the DACL or a well-known SID.
        // ConvertSidToStringSidW allocates `wide`; LocalFree releases it.
        unsafe {
            ConvertSidToStringSidW(PSID(sid.as_ptr() as *mut _), &mut wide)
                .map_err(windows_error)?;
            let text = wide
                .to_string()
                .map_err(|err| ServiceError::Windows(err.to_string()))?;
            LocalFree(Some(HLOCAL(wide.as_ptr() as *mut _)));
            Ok(text)
        }
    }

    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Vec<u8>, ServiceError> {
        let mut storage = vec![0u64; (SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<u64>())];
        let mut len = SECURITY_MAX_SID_SIZE;
        // SAFETY: `storage` is large enough for SECURITY_MAX_SID_SIZE and aligned.
        unsafe {
            CreateWellKnownSid(
                kind,
                None,
                Some(PSID(storage.as_mut_ptr() as *mut _)),
                &mut len,
            )
            .map_err(windows_error)?;
            let mut bytes = vec![0u8; len as usize];
            std::ptr::copy_nonoverlapping(
                storage.as_ptr() as *const u8,
                bytes.as_mut_ptr(),
                bytes.len(),
            );
            Ok(bytes)
        }
    }

    struct SdGuard(PSECURITY_DESCRIPTOR);

    impl Drop for SdGuard {
        fn drop(&mut self) {
            if !self.0.0.is_null() {
                // SAFETY: this pointer was allocated by GetSecurityInfo.
                unsafe {
                    LocalFree(Some(HLOCAL(self.0.0)));
                }
            }
        }
    }
}
