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

//! Small FFI helpers shared by the technique modules.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// NUL-terminated UTF-16 string for `*W` APIs.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Closes a kernel handle on drop.
pub struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    /// Wraps `h`, returning `None` for NULL and INVALID_HANDLE_VALUE.
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// `LoadLibraryW(dll)` + `GetProcAddress(name)`, the way the original resolves
/// psapi, ntdll and the file-cache APIs at runtime. The module is never freed,
/// which matches the original.
pub fn resolve(dll: &str, name: &str) -> Option<*const c_void> {
    let dll = wide(dll);
    let name: Vec<u8> = name.bytes().chain(std::iter::once(0)).collect();
    unsafe {
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, name.as_ptr()).map(|f| f as *const c_void)
    }
}

pub fn format_kb(kb: i64) -> String {
    let sign = if kb < 0 { "-" } else { "" };
    let kb = kb.unsigned_abs() as f64;
    if kb >= 1024.0 * 1024.0 {
        format!("{sign}{:.2} GB", kb / (1024.0 * 1024.0))
    } else if kb >= 1024.0 {
        format!("{sign}{:.2} MB", kb / 1024.0)
    } else {
        format!("{sign}{kb:.0} KB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_is_nul_terminated() {
        assert_eq!(wide("A:"), vec![b'A' as u16, b':' as u16, 0]);
    }

    #[test]
    fn format_kb_units() {
        assert_eq!(format_kb(512), "512 KB");
        assert_eq!(format_kb(2048), "2.00 MB");
        assert_eq!(format_kb(3 * 1024 * 1024), "3.00 GB");
        assert_eq!(format_kb(-2048), "-2.00 MB");
    }
}
