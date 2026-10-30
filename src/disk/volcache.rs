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

//! The Disk Cleanup handlers of Windows (`IEmptyVolumeCache`), the same ones that
//! Disk Cleanup and Storage Sense use. Each one is registered under
//! `HKLM\...\Explorer\VolumeCaches\<name>` with its CLSID as the default value.
//! Calling them is the supported way to remove Windows Update leftovers, old
//! Windows installations and other system files.
//!
//! The interfaces are declared by hand from `emptyvc.h`; windows-sys has no COM
//! vtables. Only handlers named by `clean::Category` are ever loaded.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
};

use crate::win::wide;

const ROOT: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VolumeCaches";

const S_OK: HRESULT = 0;
const S_FALSE: HRESULT = 1;
const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
const E_ABORT: HRESULT = 0x8000_4004_u32 as i32;
const COINIT_APARTMENTTHREADED: u32 = 0x2;

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_CACHE: GUID = GUID::from_u128(0x8fce5227_04da_11d1_a004_00805f8abe06);
const IID_CACHE2: GUID = GUID::from_u128(0x02b7e3ba_4db3_11d2_b2d9_00c04f8eec8c);
const IID_CALLBACK: GUID = GUID::from_u128(0x6e793361_73c6_11d0_8469_00aa00442901);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not registered on this Windows version.
    Missing,
    /// The handler could not be created or refused to start.
    Failed,
    Cancelled,
}

type Hr = HRESULT;

#[repr(C)]
struct CacheVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> Hr,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    initialize: unsafe extern "system" fn(
        *mut c_void,
        HKEY,
        PCWSTR,
        *mut PWSTR,
        *mut PWSTR,
        *mut u32,
    ) -> Hr,
    get_space_used: unsafe extern "system" fn(*mut c_void, *mut u64, *mut c_void) -> Hr,
    purge: unsafe extern "system" fn(*mut c_void, u64, *mut c_void) -> Hr,
    show_properties: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hr,
    deactivate: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hr,
    /// IEmptyVolumeCache2 only.
    initialize_ex: unsafe extern "system" fn(
        *mut c_void,
        HKEY,
        PCWSTR,
        PCWSTR,
        *mut PWSTR,
        *mut PWSTR,
        *mut PWSTR,
        *mut u32,
    ) -> Hr,
}

/// `IEmptyVolumeCacheCallBack`: lets a long scan or purge be cancelled and
/// reports how much was freed.
#[repr(C)]
struct Callback {
    vtbl: *const CallbackVtbl,
    cancel: *const AtomicBool,
    freed: AtomicU64,
}

#[repr(C)]
struct CallbackVtbl {
    query_interface: unsafe extern "system" fn(*mut Callback, *const GUID, *mut *mut c_void) -> Hr,
    add_ref: unsafe extern "system" fn(*mut Callback) -> u32,
    release: unsafe extern "system" fn(*mut Callback) -> u32,
    scan_progress: unsafe extern "system" fn(*mut Callback, u64, u32, PCWSTR) -> Hr,
    purge_progress: unsafe extern "system" fn(*mut Callback, u64, u64, u32, PCWSTR) -> Hr,
}

fn same(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

unsafe extern "system" fn cb_query(
    this: *mut Callback,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> Hr {
    if same(&*iid, &IID_IUNKNOWN) || same(&*iid, &IID_CALLBACK) {
        *out = this.cast();
        S_OK
    } else {
        *out = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

// The callback lives on the caller's stack for the whole call; counting is not needed.
unsafe extern "system" fn cb_add_ref(_: *mut Callback) -> u32 {
    1
}

unsafe extern "system" fn cb_release(_: *mut Callback) -> u32 {
    1
}

unsafe fn cb_status(this: *mut Callback) -> Hr {
    if (*(*this).cancel).load(Ordering::Relaxed) {
        E_ABORT
    } else {
        S_OK
    }
}

unsafe extern "system" fn cb_scan(this: *mut Callback, _: u64, _: u32, _: PCWSTR) -> Hr {
    cb_status(this)
}

unsafe extern "system" fn cb_purge(
    this: *mut Callback,
    freed: u64,
    _: u64,
    _: u32,
    _: PCWSTR,
) -> Hr {
    (*this).freed.fetch_max(freed, Ordering::Relaxed);
    cb_status(this)
}

static CALLBACK_VTBL: CallbackVtbl = CallbackVtbl {
    query_interface: cb_query,
    add_ref: cb_add_ref,
    release: cb_release,
    scan_progress: cb_scan,
    purge_progress: cb_purge,
};

/// COM for the current thread, released on drop.
struct Apartment(bool);

impl Apartment {
    fn enter() -> Self {
        Apartment(unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED) } >= 0)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn open_key(name: &str) -> Option<RegKey> {
    let path = wide(&format!("{ROOT}\\{name}"));
    let mut key: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) };
    (rc == 0).then_some(RegKey(key))
}

fn default_value(key: &RegKey) -> Option<String> {
    let mut buf = [0u16; 128];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    let rc = unsafe {
        RegQueryValueExW(
            key.0,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = (bytes as usize / 2).min(buf.len());
    let s = &buf[..len];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Some(String::from_utf16_lossy(&s[..end]))
}

/// "{8FCE5227-04DA-11d1-A004-00805F8ABE06}" (braces optional) into a GUID.
pub fn parse_guid(s: &str) -> Option<GUID> {
    let s = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if parts.len() != 5 || parts.iter().zip(lens).any(|(p, l)| p.len() != l) {
        return None;
    }
    let hex: String = parts.concat();
    u128::from_str_radix(&hex, 16).ok().map(GUID::from_u128)
}

/// Whether the handler `name` is registered here.
pub fn exists(name: &str) -> bool {
    open_key(name)
        .and_then(|k| default_value(&k))
        .and_then(|v| parse_guid(&v))
        .is_some()
}

/// A created and initialized handler, released (after `Deactivate`) on drop.
struct Handler {
    this: *mut c_void,
    _key: RegKey,
}

impl Handler {
    fn vtbl(&self) -> &CacheVtbl {
        unsafe { &**(self.this as *const *const CacheVtbl) }
    }

    /// `Ok(None)`: the handler has nothing to do on this drive.
    fn open(name: &str, volume: &str) -> Result<Option<Handler>, Error> {
        let key = open_key(name).ok_or(Error::Missing)?;
        let clsid = default_value(&key)
            .and_then(|v| parse_guid(&v))
            .ok_or(Error::Missing)?;
        let mut this: *mut c_void = std::ptr::null_mut();
        let mut v2 = true;
        let mut hr = unsafe {
            CoCreateInstance(
                &clsid,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_CACHE2,
                &mut this,
            )
        };
        if hr < 0 {
            v2 = false;
            hr = unsafe {
                CoCreateInstance(
                    &clsid,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_CACHE,
                    &mut this,
                )
            };
        }
        if hr < 0 || this.is_null() {
            return Err(Error::Failed);
        }
        let handler = Handler { this, _key: key };
        let vol = wide(volume);
        let key_name = wide(name);
        let (mut display, mut desc, mut button): (PWSTR, PWSTR, PWSTR) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let mut flags = 0u32;
        let v = handler.vtbl();
        let hr = unsafe {
            if v2 {
                (v.initialize_ex)(
                    this,
                    handler._key.0,
                    vol.as_ptr(),
                    key_name.as_ptr(),
                    &mut display,
                    &mut desc,
                    &mut button,
                    &mut flags,
                )
            } else {
                (v.initialize)(
                    this,
                    handler._key.0,
                    vol.as_ptr(),
                    &mut display,
                    &mut desc,
                    &mut flags,
                )
            }
        };
        for p in [display, desc, button] {
            if !p.is_null() {
                unsafe { CoTaskMemFree(p as *const c_void) };
            }
        }
        match hr {
            S_OK => Ok(Some(handler)),
            S_FALSE => Ok(None),
            _ => Err(Error::Failed),
        }
    }

    fn space_used(&self, cb: &mut Callback) -> Result<u64, Error> {
        let mut used = 0u64;
        let hr = unsafe {
            (self.vtbl().get_space_used)(self.this, &mut used, (cb as *mut Callback).cast())
        };
        match hr {
            E_ABORT => Err(Error::Cancelled),
            // S_FALSE: the size could not be worked out fully; it is still the best guess.
            h if h >= 0 => Ok(used),
            _ => Err(Error::Failed),
        }
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        let mut flags = 0u32;
        unsafe {
            (self.vtbl().deactivate)(self.this, &mut flags);
            (self.vtbl().release)(self.this);
        }
    }
}

fn callback(cancel: &AtomicBool) -> Callback {
    Callback {
        vtbl: &CALLBACK_VTBL,
        cancel,
        freed: AtomicU64::new(0),
    }
}

/// Bytes the handler `name` can free on `volume` ("C:\").
pub fn space_used(name: &str, volume: &str, cancel: &AtomicBool) -> Result<u64, Error> {
    let _com = Apartment::enter();
    let Some(h) = Handler::open(name, volume)? else {
        return Ok(0);
    };
    h.space_used(&mut callback(cancel))
}

/// Lets the handler `name` remove its files on `volume`. Returns the bytes freed as
/// the handler reports them, or its own estimate if it reports nothing.
pub fn purge(name: &str, volume: &str, cancel: &AtomicBool) -> Result<u64, Error> {
    let _com = Apartment::enter();
    let Some(h) = Handler::open(name, volume)? else {
        return Ok(0);
    };
    let mut cb = callback(cancel);
    // Handlers expect GetSpaceUsed before Purge, as Disk Cleanup does.
    let used = h.space_used(&mut cb)?;
    if used == 0 {
        return Ok(0);
    }
    let hr = unsafe { (h.vtbl().purge)(h.this, used, (&mut cb as *mut Callback).cast()) };
    let reported = cb.freed.load(Ordering::Relaxed);
    match hr {
        E_ABORT => Err(Error::Cancelled),
        h if h >= 0 => Ok(if reported > 0 {
            reported.min(used)
        } else {
            used
        }),
        _ => Err(Error::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_guids() {
        let g = parse_guid("{8FCE5227-04DA-11d1-A004-00805F8ABE06}").unwrap();
        assert!(same(&g, &IID_CACHE));
        assert!(parse_guid("02b7e3ba-4db3-11d2-b2d9-00c04f8eec8c").is_some());
        assert!(parse_guid("{not-a-guid}").is_none());
        assert!(parse_guid("{8FCE5227-04DA-11d1-A004-00805F8ABE0}").is_none());
    }

    #[test]
    fn finds_registered_handlers() {
        assert!(exists("Thumbnail Cache"));
        assert!(!exists("No Such Handler 12345"));
        let cancel = AtomicBool::new(false);
        assert_eq!(
            space_used("No Such Handler 12345", "C:\\", &cancel),
            Err(Error::Missing)
        );
    }

    #[test]
    fn measures_without_deleting() {
        // The thumbnail handler works without administrator rights.
        let cancel = AtomicBool::new(false);
        let drive = format!("{}:\\", crate::disk::volume::system_letter());
        assert!(space_used("Thumbnail Cache", &drive, &cancel).is_ok());
    }
}
