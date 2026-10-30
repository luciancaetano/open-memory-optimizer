// OpenMemoryOptimizer - a simple memory optimizer for Windows
// Copyright (C) 2026 Lucian Caetano
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Token privilege handling (original: helper at 0x79a740, called from 0xc8d000).

use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_SUCCESS, HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation,
    LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::win::{wide, OwnedHandle};

/// Enabled once at startup, in this order, on the process token.
pub const STARTUP_PRIVILEGES: [&str; 4] = [
    "SeDebugPrivilege",
    "SeBackupPrivilege",
    "SeRestorePrivilege",
    "SeProfileSingleProcessPrivilege",
];

/// Needed by `SetSystemFileCacheSize`; enabled just before the file-cache step.
pub const INCREASE_QUOTA: &str = "SeIncreaseQuotaPrivilege";

fn open_own_token(access: u32) -> Option<OwnedHandle> {
    let mut token: HANDLE = null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) };
    if ok == 0 {
        None
    } else {
        OwnedHandle::new(token)
    }
}

/// Port of 0x79a740: LookupPrivilegeValueW + AdjustTokenPrivileges, success
/// judged by `GetLastError() == ERROR_SUCCESS` (AdjustTokenPrivileges returns
/// TRUE with ERROR_NOT_ALL_ASSIGNED when the token lacks the privilege).
fn set_privilege(token: HANDLE, name: &str, enable: bool) -> bool {
    let name = wide(name);
    let mut tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: LUID {
                LowPart: 0,
                HighPart: 0,
            },
            Attributes: if enable { SE_PRIVILEGE_ENABLED } else { 0 },
        }],
    };
    unsafe {
        if LookupPrivilegeValueW(null(), name.as_ptr(), &mut tp.Privileges[0].Luid) == 0 {
            return false;
        }
        let mut ret_len = 0u32;
        AdjustTokenPrivileges(
            token,
            0,
            &tp,
            size_of::<TOKEN_PRIVILEGES>() as u32,
            null_mut(),
            &mut ret_len,
        );
        GetLastError() == ERROR_SUCCESS
    }
}

/// Opens the process token with TOKEN_ADJUST_PRIVILEGES and enables each name.
pub fn enable(names: &[&'static str]) -> Vec<(&'static str, bool)> {
    let Some(token) = open_own_token(TOKEN_ADJUST_PRIVILEGES) else {
        return names.iter().map(|n| (*n, false)).collect();
    };
    names
        .iter()
        .map(|n| (*n, set_privilege(token.0, n, true)))
        .collect()
}

pub fn is_elevated() -> bool {
    let Some(token) = open_own_token(TOKEN_QUERY) else {
        return false;
    };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok != 0 && elevation.TokenIsElevated != 0
}
