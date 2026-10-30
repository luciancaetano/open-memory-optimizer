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

//! The five memory techniques, each a port of one routine in the original.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{FALSE, GENERIC_READ, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, PAGE_NOCACHE, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, SetProcessWorkingSetSize, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
};

use crate::privilege;
use crate::process;
use crate::win::{resolve, wide, OwnedHandle};

// ---------------------------------------------------------------------------
// 1. Clipboard (worker 0xc8d975: Clipboard.Clear when ClearBoard is set)
// ---------------------------------------------------------------------------

pub fn clear_clipboard() -> bool {
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return false;
        }
        let ok = EmptyClipboard() != 0;
        CloseClipboard();
        ok
    }
}

// ---------------------------------------------------------------------------
// 2. "Defrag memory" (0xc75870)
//    Commit N one-megabyte blocks (PAGE_READWRITE | PAGE_NOCACHE), zero each
//    one so it is actually backed by physical pages, then release them all.
//    The memory pressure makes the memory manager trim other working sets.
// ---------------------------------------------------------------------------

pub const DEFRAG_BLOCK: usize = 0x10_0000;
pub const DEFRAG_MAX_BLOCKS: u32 = 1000;

/// Worker 0xc8da0c: `Round(AvailPhys / 1 MiB * 0.5)`, capped at 1000.
pub fn defrag_block_count(avail_phys: u64) -> u32 {
    let n = (avail_phys as f64 / 1_048_576.0 * 0.5).round_ties_even() as i64;
    n.clamp(0, DEFRAG_MAX_BLOCKS as i64) as u32
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefragStats {
    pub requested: u32,
    pub committed: u32,
}

/// `stop` is polled before each block; blocks committed so far are still released.
pub fn defrag_memory(blocks: u32, stop: &dyn Fn() -> bool) -> Option<DefragStats> {
    // The original guards re-entry with a global flag (0xda9b14).
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::Acquire) {
        return None;
    }
    let mut stats = DefragStats {
        requested: blocks,
        committed: 0,
    };
    // SetLength(blocks + 1), zero-initialised.
    let mut ptrs: Vec<*mut c_void> = vec![null_mut(); blocks as usize + 1];
    for slot in ptrs.iter_mut().take(blocks as usize) {
        if stop() {
            break;
        }
        let p = unsafe {
            VirtualAlloc(
                null(),
                DEFRAG_BLOCK,
                MEM_COMMIT,
                PAGE_READWRITE | PAGE_NOCACHE,
            )
        };
        if !p.is_null() {
            *slot = p;
            // FillChar(p^, $100000, 0): touch every page.
            unsafe { std::ptr::write_bytes(p.cast::<u8>(), 0, DEFRAG_BLOCK) };
            std::hint::black_box(p);
            stats.committed += 1;
        }
    }
    for p in ptrs.into_iter().take(blocks as usize) {
        if !p.is_null() {
            unsafe { VirtualFree(p, 0, MEM_RELEASE) };
        }
    }
    BUSY.store(false, Ordering::Release);
    Some(stats)
}

// ---------------------------------------------------------------------------
// 3. Working-set trim of every process (0xc8d170)
//    Toolhelp snapshot -> OpenProcess(PROCESS_ALL_ACCESS, TRUE, pid)
//    -> SetProcessWorkingSetSize(h, -1, -1) -> psapi!EmptyWorkingSet(h)
//    Divergence: the handle asks only for what the two calls need (TRIM_ACCESS)
//    and is not inheritable. Full access (memory read/write, thread creation) is
//    what anti-cheat software watches for, and it was never used.
// ---------------------------------------------------------------------------

/// Rights `SetProcessWorkingSetSize` and `EmptyWorkingSet` require.
pub const TRIM_ACCESS: u32 = PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION;

type EmptyWorkingSet = unsafe extern "system" fn(HANDLE) -> i32;

/// The original resolves psapi dynamically (0x467810 loads "PSAPI.dll").
fn empty_working_set_fn() -> Option<EmptyWorkingSet> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    F.get_or_init(|| resolve("PSAPI.dll", "EmptyWorkingSet").map(|p| p as usize))
        .map(|p| unsafe { std::mem::transmute::<usize, EmptyWorkingSet>(p) })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TrimStats {
    pub processes: u32,
    pub opened: u32,
    pub set_ws_ok: u32,
    pub empty_ws_ok: u32,
}

/// `stop` is polled before each process. Processes in `skip` (a boosted game and
/// its helpers) and anti-cheat processes are left alone; the original trims every
/// process.
pub fn trim_all_working_sets(stop: &dyn Fn() -> bool, skip: &[u32]) -> std::io::Result<TrimStats> {
    let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        .ok_or_else(std::io::Error::last_os_error)?;
    let empty_ws = empty_working_set_fn();
    let mut stats = TrimStats::default();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32; // 0x238, as in the original
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more && !stop() {
        let name_len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
        let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
        if skip.contains(&entry.th32ProcessID) || process::is_anti_cheat(&name) {
            more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
            continue;
        }
        stats.processes += 1;
        // The original asks for PROCESS_ALL_ACCESS with bInheritHandle TRUE.
        let h = unsafe { OpenProcess(TRIM_ACCESS, FALSE, entry.th32ProcessID) };
        if let Some(process) = OwnedHandle::new(h) {
            stats.opened += 1;
            if unsafe { SetProcessWorkingSetSize(process.0, usize::MAX, usize::MAX) } != 0 {
                stats.set_ws_ok += 1;
            }
            if let Some(f) = empty_ws {
                if unsafe { f(process.0) } != 0 {
                    stats.empty_ws_ok += 1;
                }
            }
        }
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(stats)
}

// ---------------------------------------------------------------------------
// 4. Standby-list purge (0xc8e170)
//    NtSetSystemInformation(SystemMemoryListInformation = 80,
//                           &MemoryPurgeStandbyList (= 4), sizeof(u32))
//    Requires SeProfileSingleProcessPrivilege (enabled at startup).
// ---------------------------------------------------------------------------

pub const SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;
pub const MEMORY_PURGE_STANDBY_LIST: u32 = 4;

type NtSetSystemInformation = unsafe extern "system" fn(u32, *mut c_void, u32) -> i32;

/// Returns the NTSTATUS (the original ignores it), or `None` if ntdll exports
/// are missing. Like the original, both NtSet- and NtQuerySystemInformation
/// must resolve before the call is made.
pub fn purge_standby_list() -> Option<i32> {
    let set = resolve("ntdll.dll", "NtSetSystemInformation")?;
    resolve("ntdll.dll", "NtQuerySystemInformation")?;
    let set: NtSetSystemInformation = unsafe { std::mem::transmute(set) };
    let mut command = MEMORY_PURGE_STANDBY_LIST;
    Some(unsafe {
        set(
            SYSTEM_MEMORY_LIST_INFORMATION,
            (&mut command as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    })
}

// ---------------------------------------------------------------------------
// 5. System file cache (0xc8d580) - runs on every optimization
//    SeIncreaseQuotaPrivilege -> GetSystemFileCacheSize -> open+flush every
//    drive root A:\..Z:\ -> SetSystemFileCacheSize(0, 0, flags)
// ---------------------------------------------------------------------------

type GetSystemFileCacheSize = unsafe extern "system" fn(*mut usize, *mut usize, *mut u32) -> i32;
type SetSystemFileCacheSize = unsafe extern "system" fn(usize, usize, u32) -> i32;

pub const FILE_CACHE_MAX_HARD_ENABLE: u32 = 0x1;
pub const FILE_CACHE_MIN_HARD_ENABLE: u32 = 0x4;

#[derive(Debug, Default, Clone)]
pub struct FileCacheReport {
    pub privilege_enabled: bool,
    pub min: usize,
    pub max: usize,
    pub flags: u32,
    pub roots_opened: Vec<char>,
    pub roots_flushed: Vec<char>,
    /// `None` when the set call was skipped.
    pub set_ok: Option<bool>,
}

pub fn trim_file_cache() -> Result<FileCacheReport, &'static str> {
    let get = resolve("kernel32.dll", "GetSystemFileCacheSize")
        .ok_or("GetSystemFileCacheSize missing")?;
    let set = resolve("kernel32.dll", "SetSystemFileCacheSize")
        .ok_or("SetSystemFileCacheSize missing")?;
    let get: GetSystemFileCacheSize = unsafe { std::mem::transmute(get) };
    let set: SetSystemFileCacheSize = unsafe { std::mem::transmute(set) };

    let mut report = FileCacheReport {
        privilege_enabled: privilege::enable(&[privilege::INCREASE_QUOTA])[0].1,
        flags: 1, // initial value in the original before the query overwrites it
        ..Default::default()
    };
    if unsafe { get(&mut report.min, &mut report.max, &mut report.flags) } == 0 {
        return Err("GetSystemFileCacheSize failed");
    }

    // Original: CreateFileW("X:\", GENERIC_READ, FILE_SHARE_READ|FILE_SHARE_WRITE,
    //                       nil, OPEN_EXISTING, 0, 0) then FlushFileBuffers.
    // Without FILE_FLAG_BACKUP_SEMANTICS a directory open normally fails, so
    // this loop is expected to flush nothing; it is kept for fidelity.
    for letter in 'A'..='Z' {
        let path = wide(&format!("{letter}:\\"));
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if let Some(h) = OwnedHandle::new(h) {
            report.roots_opened.push(letter);
            if unsafe { FlushFileBuffers(h.0) } != 0 {
                report.roots_flushed.push(letter);
            }
        }
    }

    // Safety divergence: with a hard cache limit configured, Set(0, 0, flags)
    // would ask for a zero-size hard cap, so leave such systems alone.
    if report.flags & (FILE_CACHE_MAX_HARD_ENABLE | FILE_CACHE_MIN_HARD_ENABLE) != 0 {
        return Ok(report);
    }
    report.set_ok = Some(unsafe { set(0, 0, report.flags) } != 0);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defrag_count_is_half_of_free_mb_capped() {
        assert_eq!(defrag_block_count(0), 0);
        assert_eq!(defrag_block_count(512 * 1_048_576), 256);
        assert_eq!(defrag_block_count(3 * 1_048_576), 2); // 1.5 -> 2
        assert_eq!(defrag_block_count(5 * 1_048_576), 2); // 2.5 -> 2 (ties to even)
        assert_eq!(defrag_block_count(16 * 1024 * 1_048_576), 1000);
    }

    #[test]
    fn defrag_small_run_commits_and_releases() {
        let stats = defrag_memory(4, &|| false).unwrap();
        assert_eq!(stats.requested, 4);
        assert_eq!(stats.committed, 4);

        // Stopping after two blocks; same test because of the BUSY guard.
        let polls = std::cell::Cell::new(0);
        let stop = || {
            polls.set(polls.get() + 1);
            polls.get() > 2
        };
        let stats = defrag_memory(4, &stop).unwrap();
        assert_eq!((stats.requested, stats.committed), (4, 2));
    }

    #[test]
    fn trim_access_is_enough_to_trim() {
        let h = OwnedHandle::new(unsafe { OpenProcess(TRIM_ACCESS, FALSE, std::process::id()) })
            .unwrap();
        assert_ne!(
            unsafe { SetProcessWorkingSetSize(h.0, usize::MAX, usize::MAX) },
            0
        );
        assert_ne!(unsafe { empty_working_set_fn().unwrap()(h.0) }, 0);
    }

    #[test]
    fn psapi_and_ntdll_exports_resolve() {
        assert!(empty_working_set_fn().is_some());
        assert!(resolve("ntdll.dll", "NtSetSystemInformation").is_some());
        assert!(resolve("kernel32.dll", "SetSystemFileCacheSize").is_some());
    }
}
