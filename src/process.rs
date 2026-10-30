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

//! Process list and per-process statistics for the Game Booster page. Not in the
//! original.
//!
//! CPU, memory, disk and handle numbers come from the process handle; GPU use and
//! video memory come from the PDH `GPU Engine` and `GPU Process Memory` counters,
//! whose instance names start with `pid_<N>_`.

use std::collections::HashMap;
use std::mem::{size_of, zeroed};
use std::time::Instant;

use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_MORE_DATA,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessHandleCount, GetProcessIoCounters, GetProcessTimes, OpenProcess,
    IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
};

use crate::win::{wide, OwnedHandle};

/// One entry of the process list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: u32,
    /// Executable name, e.g. `game.exe`.
    pub name: String,
    pub threads: u32,
    /// Title of the process's main window, if it has a visible one.
    pub title: Option<String>,
}

impl ProcInfo {
    /// Window title when there is one, otherwise the executable name.
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

fn utf16_until_nul(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Every process from a Toolhelp snapshot, without window titles.
pub fn snapshot() -> Vec<ProcInfo> {
    let Some(snap) = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snap.0, &mut entry) } != 0;
    while more {
        out.push(ProcInfo {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: utf16_until_nul(&entry.szExeFile),
            threads: entry.cntThreads,
            title: None,
        });
        more = unsafe { Process32NextW(snap.0, &mut entry) } != 0;
    }
    out
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    let titles = &mut *(lparam as *mut HashMap<u32, String>);
    let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
    if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() || tool {
        return 1;
    }
    let mut buf = [0u16; 256];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        return 1;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    // Keep the longest title: games often own a small helper window too.
    let title = utf16_until_nul(&buf[..n as usize]);
    let slot = titles.entry(pid).or_default();
    if title.len() > slot.len() {
        *slot = title;
    }
    1
}

/// Main-window title of each process that has a visible top-level window.
pub fn window_titles() -> HashMap<u32, String> {
    let mut titles: HashMap<u32, String> = HashMap::new();
    unsafe { EnumWindows(Some(collect_window), &mut titles as *mut _ as LPARAM) };
    titles
}

/// Processes for the picker: apps with a window first (by title), then the rest by name.
pub fn list() -> Vec<ProcInfo> {
    let titles = window_titles();
    let mut procs: Vec<ProcInfo> = snapshot()
        .into_iter()
        .filter(|p| p.pid > 4)
        .map(|mut p| {
            p.title = titles.get(&p.pid).cloned();
            p
        })
        .collect();
    sort_for_picker(&mut procs);
    procs
}

pub fn sort_for_picker(procs: &mut [ProcInfo]) {
    procs.sort_by_cached_key(|p| (p.title.is_none(), p.display_name().to_lowercase(), p.pid));
}

/// Case-insensitive match on the executable name or window title.
pub fn matches(p: &ProcInfo, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || p.name.to_lowercase().contains(&f)
        || p.title
            .as_ref()
            .is_some_and(|t| t.to_lowercase().contains(&f))
}

pub fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

fn filetime(ft: FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

/// Creation time (100 ns units), CPU time used (kernel + user) and whether it
/// still runs. The creation time tells a process apart from a later one that got
/// the same PID.
pub struct Times {
    pub created: u64,
    pub cpu: u64,
}

pub fn times(h: &OwnedHandle) -> Option<Times> {
    let mut c: FILETIME = unsafe { zeroed() };
    let (mut e, mut k, mut u) = (c, c, c);
    if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } == 0 {
        return None;
    }
    Some(Times {
        created: filetime(c),
        cpu: filetime(k) + filetime(u),
    })
}

pub fn open(pid: u32, access: u32) -> Option<OwnedHandle> {
    OwnedHandle::new(unsafe { OpenProcess(access, 0, pid) })
}

pub fn is_running(h: &OwnedHandle) -> bool {
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(h.0, &mut code) != 0 && code == STILL_ACTIVE as u32 }
}

/// Creation time of `pid`, or `None` when it cannot be opened.
pub fn creation_time(pid: u32) -> Option<u64> {
    open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
        .and_then(|h| times(&h))
        .map(|t| t.created)
}

fn now_filetime() -> u64 {
    let mut ft: FILETIME = unsafe { zeroed() };
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    filetime(ft)
}

/// One sample of a process.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcStats {
    /// Share of the whole processor, 0-100, like Task Manager.
    pub cpu_pct: f32,
    pub working_set: u64,
    pub private_bytes: u64,
    pub page_faults_per_sec: f64,
    pub read_bps: f64,
    pub write_bps: f64,
    pub threads: u32,
    pub handles: u32,
    pub uptime_secs: u64,
    /// Busiest graphics engine used by the process, 0-100.
    pub gpu_pct: Option<f32>,
    /// Dedicated video memory in use.
    pub vram: Option<u64>,
}

/// `cpu` time used in `wall` time, both in 100 ns units, as a share of `ncpu` processors.
pub fn cpu_percent(cpu: u64, wall: u64, ncpu: u32) -> f32 {
    if wall == 0 || ncpu == 0 {
        return 0.0;
    }
    (cpu as f64 / wall as f64 / ncpu as f64 * 100.0).clamp(0.0, 100.0) as f32
}

struct Prev {
    at: Instant,
    cpu: u64,
    read: u64,
    write: u64,
    faults: u32,
}

/// Samples one process; call [`ProcessMonitor::sample`] about once a second.
pub struct ProcessMonitor {
    pub pid: u32,
    handle: OwnedHandle,
    pub created: u64,
    ncpu: u32,
    prev: Option<Prev>,
    gpu: Option<GpuCounters>,
}

impl ProcessMonitor {
    pub fn open(pid: u32) -> Option<Self> {
        let handle = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let created = times(&handle)?.created;
        Some(Self {
            pid,
            handle,
            created,
            ncpu: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            prev: None,
            gpu: GpuCounters::open(),
        })
    }

    pub fn is_running(&self) -> bool {
        is_running(&self.handle)
    }

    /// `None` once the process has exited. Rates are 0 on the first sample.
    pub fn sample(&mut self) -> Option<ProcStats> {
        if !self.is_running() {
            return None;
        }
        let now = Instant::now();
        let t = times(&self.handle)?;
        let mut mem: PROCESS_MEMORY_COUNTERS_EX = unsafe { zeroed() };
        mem.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        unsafe { GetProcessMemoryInfo(self.handle.0, &mut mem as *mut _ as *mut _, mem.cb) };
        let mut io: IO_COUNTERS = unsafe { zeroed() };
        unsafe { GetProcessIoCounters(self.handle.0, &mut io) };
        let mut handles = 0;
        unsafe { GetProcessHandleCount(self.handle.0, &mut handles) };
        let threads = snapshot()
            .iter()
            .find(|p| p.pid == self.pid)
            .map_or(0, |p| p.threads);

        let mut s = ProcStats {
            working_set: mem.WorkingSetSize as u64,
            private_bytes: mem.PrivateUsage as u64,
            threads,
            handles,
            uptime_secs: now_filetime().saturating_sub(t.created) / 10_000_000,
            ..Default::default()
        };
        if let Some(p) = &self.prev {
            let secs = (now - p.at).as_secs_f64().max(0.001);
            let wall = (secs * 10_000_000.0) as u64;
            s.cpu_pct = cpu_percent(t.cpu.saturating_sub(p.cpu), wall, self.ncpu);
            s.read_bps = io.ReadTransferCount.saturating_sub(p.read) as f64 / secs;
            s.write_bps = io.WriteTransferCount.saturating_sub(p.write) as f64 / secs;
            s.page_faults_per_sec = mem.PageFaultCount.wrapping_sub(p.faults) as f64 / secs;
        }
        self.prev = Some(Prev {
            at: now,
            cpu: t.cpu,
            read: io.ReadTransferCount,
            write: io.WriteTransferCount,
            faults: mem.PageFaultCount,
        });
        if let Some(gpu) = &mut self.gpu {
            if let Some((util, vram)) = gpu.collect() {
                s.gpu_pct = Some(gpu_percent(&util, self.pid) as f32);
                s.vram = Some(vram_bytes(&vram, self.pid));
            }
        }
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// GPU counters
// ---------------------------------------------------------------------------

/// PID in a `GPU Engine` / `GPU Process Memory` instance name such as
/// `pid_1234_luid_0x00000000_0x0000C739_phys_0_eng_3_engtype_3D`.
pub fn instance_pid(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}

/// Busiest engine of `pid`, as Task Manager shows it.
pub fn gpu_percent(items: &[(String, f64)], pid: u32) -> f64 {
    items
        .iter()
        .filter(|(n, _)| instance_pid(n) == Some(pid))
        .map(|(_, v)| *v)
        .fold(0.0, f64::max)
        .clamp(0.0, 100.0)
}

/// Dedicated video memory of `pid` over all adapters.
pub fn vram_bytes(items: &[(String, f64)], pid: u32) -> u64 {
    items
        .iter()
        .filter(|(n, _)| instance_pid(n) == Some(pid))
        .map(|(_, v)| *v as u64)
        .sum()
}

/// `(instance name, value)` pairs of a wildcard counter.
type Instances = Vec<(String, f64)>;

struct GpuCounters {
    query: isize,
    engine: isize,
    memory: isize,
}

// PDH handles are plain values and may be used from any thread.
unsafe impl Send for GpuCounters {}

impl GpuCounters {
    fn open() -> Option<Self> {
        unsafe {
            let mut query = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let add = |path: &str| {
                let path = wide(path);
                let mut c = 0;
                (PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut c) == 0).then_some(c)
            };
            match (
                add(r"\GPU Engine(*)\Utilization Percentage"),
                add(r"\GPU Process Memory(*)\Dedicated Usage"),
            ) {
                (Some(engine), Some(memory)) => {
                    // Utilization is a rate: prime it so the next collect has a value.
                    PdhCollectQueryData(query);
                    Some(Self {
                        query,
                        engine,
                        memory,
                    })
                }
                _ => {
                    PdhCloseQuery(query);
                    None
                }
            }
        }
    }

    /// Engine utilization and dedicated memory, per instance.
    fn collect(&mut self) -> Option<(Instances, Instances)> {
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        Some((
            read_array(self.engine, PDH_FMT_DOUBLE)?,
            read_array(self.memory, PDH_FMT_LARGE)?,
        ))
    }
}

impl Drop for GpuCounters {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Instance names and values of a wildcard counter.
fn read_array(counter: isize, format: PDH_FMT) -> Option<Instances> {
    unsafe {
        let (mut bytes, mut count) = (0u32, 0u32);
        let status = PdhGetFormattedCounterArrayW(
            counter,
            format,
            &mut bytes,
            &mut count,
            std::ptr::null_mut(),
        );
        if status != PDH_MORE_DATA {
            // No instances at all (no GPU process yet) is not an error.
            return (status == 0).then(Vec::new);
        }
        // u64 storage keeps the items aligned; the names live after them.
        let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
        let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        if PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, items) != 0 {
            return None;
        }
        let items = std::slice::from_raw_parts(items, count as usize);
        Some(
            items
                .iter()
                .filter(|it| {
                    matches!(
                        it.FmtValue.CStatus,
                        PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                    )
                })
                .map(|it| {
                    let mut len = 0;
                    while *it.szName.add(len) != 0 {
                        len += 1;
                    }
                    let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, len));
                    let v = if format == PDH_FMT_LARGE {
                        it.FmtValue.Anonymous.largeValue as f64
                    } else {
                        it.FmtValue.Anonymous.doubleValue
                    };
                    (name, v)
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, title: Option<&str>) -> ProcInfo {
        ProcInfo {
            pid,
            parent: 0,
            name: name.into(),
            threads: 1,
            title: title.map(Into::into),
        }
    }

    #[test]
    fn cpu_percent_is_share_of_all_processors() {
        // 1 s of CPU in 1 s on 4 processors = 25 %.
        assert_eq!(cpu_percent(10_000_000, 10_000_000, 4), 25.0);
        assert_eq!(cpu_percent(5, 0, 4), 0.0);
        assert_eq!(cpu_percent(90_000_000, 10_000_000, 4), 100.0);
    }

    #[test]
    fn instance_names_give_the_pid() {
        assert_eq!(
            instance_pid("pid_1234_luid_0x00000000_0x0000C739_phys_0_eng_3_engtype_3D"),
            Some(1234)
        );
        assert_eq!(instance_pid("pid_77_luid_0x0_0x1_phys_0"), Some(77));
        assert_eq!(instance_pid("_Total"), None);
        assert_eq!(instance_pid("pid_x_luid"), None);
    }

    #[test]
    fn gpu_numbers_are_per_pid() {
        let engines = vec![
            ("pid_10_luid_a_phys_0_eng_0_engtype_3D".to_string(), 40.0),
            ("pid_10_luid_a_phys_0_eng_1_engtype_Copy".to_string(), 5.0),
            ("pid_11_luid_a_phys_0_eng_0_engtype_3D".to_string(), 90.0),
        ];
        assert_eq!(gpu_percent(&engines, 10), 40.0);
        assert_eq!(gpu_percent(&engines, 12), 0.0);
        let mem = vec![
            ("pid_10_luid_a_phys_0".to_string(), 1000.0),
            ("pid_10_luid_b_phys_0".to_string(), 24.0),
            ("pid_11_luid_a_phys_0".to_string(), 7.0),
        ];
        assert_eq!(vram_bytes(&mem, 10), 1024);
    }

    #[test]
    fn picker_puts_windows_first_and_filters() {
        let mut v = vec![
            p(3, "zeta.exe", None),
            p(1, "game.exe", Some("My Game")),
            p(2, "alpha.exe", None),
            p(4, "browser.exe", Some("Another")),
        ];
        sort_for_picker(&mut v);
        let order: Vec<u32> = v.iter().map(|p| p.pid).collect();
        assert_eq!(order, vec![4, 1, 2, 3]);
        assert!(matches(&v[1], "my g"));
        assert!(matches(&v[1], "GAME.EXE"));
        assert!(!matches(&v[2], "game"));
        assert!(matches(&v[2], "  "));
    }

    #[test]
    fn monitors_own_process() {
        let mut m = ProcessMonitor::open(std::process::id()).unwrap();
        assert!(m.is_running());
        let first = m.sample().unwrap();
        assert!(first.working_set > 0 && first.threads > 0 && first.handles > 0);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let second = m.sample().unwrap();
        assert!((0.0..=100.0).contains(&second.cpu_pct));
        assert!(list().iter().any(|p| p.pid == std::process::id()));
    }
}
