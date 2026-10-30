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

//! Game Booster: gives one process (a game) more of the machine while it runs.
//! Not in the original.
//!
//! Every change is undone by [`Session::restore`], which also runs on drop: when
//! the user clicks Restore, when the game exits and when the app closes. Changes
//! to other processes are only undone if the PID still belongs to the same
//! process (same creation time).
//!
//! Techniques:
//! 1. High priority class for the game (never realtime).
//! 2. Opt the game out of power throttling (EcoQoS), so Windows does not move it
//!    to efficiency cores or lower its clock to save energy.
//! 3. "High performance" power plan while the game runs.
//! 4. Optional: background apps of the user's session get below-normal priority,
//!    EcoQoS and low I/O priority.
//! 5. Free memory before playing: trim other processes and purge the standby list.
//! 6. Optional: on hybrid CPUs, prefer the performance cores (CPU Sets, a soft hint).
//! 7. While boosted, automatic cleanups do not trim the game.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
use windows_sys::Win32::System::Power::{PowerGetActiveScheme, PowerSetActiveScheme};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, ProcessPowerThrottling, SetPriorityClass,
    SetProcessDefaultCpuSets, SetProcessInformation, BELOW_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_SET_LIMITED_INFORMATION, REALTIME_PRIORITY_CLASS,
};

use crate::memory;
use crate::process::{self, ProcInfo};
use crate::techniques;
use crate::win::{resolve, OwnedHandle};

/// "High performance" power plan.
pub const HIGH_PERFORMANCE: GUID = GUID::from_u128(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);
/// "Ultimate performance" power plan; already as fast, so it is left alone.
pub const ULTIMATE_PERFORMANCE: GUID = GUID::from_u128(0xe9a42b02_d5df_448d_aa00_03f14749eb61);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub high_priority: bool,
    pub no_power_saving: bool,
    pub power_plan: bool,
    pub calm_background: bool,
    pub free_memory: bool,
    pub performance_cores: bool,
}

impl Options {
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        s.booster
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            high_priority: true,
            no_power_saving: true,
            power_plan: true,
            calm_background: false,
            free_memory: true,
            performance_cores: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Technique {
    Priority,
    NoPowerSaving,
    PowerPlan,
    Background,
    FreeMemory,
    PerformanceCores,
    KeepMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// Nothing to change: it was already that way.
    AlreadyOn,
    /// Windows refused (often a protected game or a missing admin right).
    Failed,
    /// This computer does not have it (no High performance plan, not a hybrid CPU).
    NotAvailable,
    /// Number of background apps calmed down.
    Calmed(u32),
    /// Memory made available, in KB (may be 0 or negative).
    Freed(i64),
}

impl Outcome {
    pub fn is_ok(self) -> bool {
        !matches!(self, Outcome::Failed | Outcome::NotAvailable)
    }
}

// ---------------------------------------------------------------------------
// 7. Keep the game's memory during automatic cleanups
// ---------------------------------------------------------------------------

static PROTECTED: AtomicU32 = AtomicU32::new(0);

/// PIDs that cleanups must not trim (the boosted game, if any).
pub fn protected() -> Vec<u32> {
    match PROTECTED.load(Ordering::Acquire) {
        0 => Vec::new(),
        pid => vec![pid],
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Opens `pid` only if it is still the process created at `created`.
fn open_same(pid: u32, created: u64, access: u32) -> Option<OwnedHandle> {
    let h = process::open(pid, access | PROCESS_QUERY_LIMITED_INFORMATION)?;
    (process::times(&h)?.created == created).then_some(h)
}

fn get_throttling(h: &OwnedHandle) -> Option<(u32, u32)> {
    let mut s: PROCESS_POWER_THROTTLING_STATE = unsafe { zeroed() };
    s.Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION;
    let ok = unsafe {
        GetProcessInformation(
            h.0,
            ProcessPowerThrottling,
            &mut s as *mut _ as *mut c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    } != 0;
    ok.then_some((s.ControlMask, s.StateMask))
}

fn set_throttling(h: &OwnedHandle, control: u32, state: u32) -> bool {
    let s = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control,
        StateMask: state,
    };
    unsafe {
        SetProcessInformation(
            h.0,
            ProcessPowerThrottling,
            &s as *const _ as *const c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

/// `ProcessIoPriority` of NtQuery/NtSetInformationProcess: 0 very low, 1 low, 2 normal.
const PROCESS_IO_PRIORITY: u32 = 33;
const IO_PRIORITY_LOW: u32 = 1;

type NtProcessInfo = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32, *mut u32) -> i32;
type NtSetProcessInfo = unsafe extern "system" fn(*mut c_void, u32, *const c_void, u32) -> i32;

fn nt_query_process() -> Option<NtProcessInfo> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    F.get_or_init(|| resolve("ntdll.dll", "NtQueryInformationProcess").map(|p| p as usize))
        .map(|p| unsafe { std::mem::transmute::<usize, NtProcessInfo>(p) })
}

fn nt_set_process() -> Option<NtSetProcessInfo> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    F.get_or_init(|| resolve("ntdll.dll", "NtSetInformationProcess").map(|p| p as usize))
        .map(|p| unsafe { std::mem::transmute::<usize, NtSetProcessInfo>(p) })
}

fn get_io_priority(h: &OwnedHandle) -> Option<u32> {
    let f = nt_query_process()?;
    let mut v = 0u32;
    let status = unsafe {
        f(
            h.0,
            PROCESS_IO_PRIORITY,
            &mut v as *mut u32 as *mut c_void,
            4,
            std::ptr::null_mut(),
        )
    };
    (status == 0).then_some(v)
}

fn set_io_priority(h: &OwnedHandle, v: u32) -> bool {
    let Some(f) = nt_set_process() else {
        return false;
    };
    unsafe {
        f(
            h.0,
            PROCESS_IO_PRIORITY,
            &v as *const u32 as *const c_void,
            4,
        ) == 0
    }
}

fn active_power_scheme() -> Option<GUID> {
    let mut p: *mut GUID = std::ptr::null_mut();
    if unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut p) } != ERROR_SUCCESS || p.is_null()
    {
        return None;
    }
    let g = unsafe { *p };
    unsafe { LocalFree(p as *mut c_void) };
    Some(g)
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

fn set_power_scheme(g: &GUID) -> bool {
    unsafe { PowerSetActiveScheme(std::ptr::null_mut(), g) == ERROR_SUCCESS }
}

/// `(cpu set id, efficiency class)` of every logical processor.
fn cpu_sets() -> Vec<(u32, u8)> {
    let mut len = 0u32;
    unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, std::ptr::null_mut(), 0)
    };
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let base = buf.as_mut_ptr().cast::<u8>();
    if unsafe { GetSystemCpuSetInformation(base.cast(), len, &mut len, std::ptr::null_mut(), 0) }
        == 0
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + size_of::<SYSTEM_CPU_SET_INFORMATION>() <= len as usize {
        let info = unsafe { &*(base.add(off) as *const SYSTEM_CPU_SET_INFORMATION) };
        if info.Size == 0 {
            break;
        }
        let set = unsafe { info.Anonymous.CpuSet };
        out.push((set.Id, set.EfficiencyClass));
        off += info.Size as usize;
    }
    out
}

/// IDs of the fastest cores, or `None` when every core is the same kind.
pub fn performance_set(sets: &[(u32, u8)]) -> Option<Vec<u32>> {
    let max = sets.iter().map(|s| s.1).max()?;
    let min = sets.iter().map(|s| s.1).min()?;
    (max != min).then(|| sets.iter().filter(|s| s.1 == max).map(|s| s.0).collect())
}

// ---------------------------------------------------------------------------
// 4. Which background processes may be calmed down
// ---------------------------------------------------------------------------

/// Windows parts and other processes that must keep their priority.
const NEVER_CALM: &[&str] = &[
    "applicationframehost.exe",
    "audiodg.exe",
    "conhost.exe",
    "csrss.exe",
    "ctfmon.exe",
    "dllhost.exe",
    "dwm.exe",
    "explorer.exe",
    "fontdrvhost.exe",
    "lockapp.exe",
    "lsass.exe",
    "memory compression",
    "openmemoryoptimizer.exe",
    "registry",
    "runtimebroker.exe",
    "searchhost.exe",
    "securityhealthsystray.exe",
    "services.exe",
    "shellexperiencehost.exe",
    "sihost.exe",
    "smss.exe",
    "startmenuexperiencehost.exe",
    "svchost.exe",
    "system",
    "taskhostw.exe",
    "taskmgr.exe",
    "textinputhost.exe",
    "wininit.exe",
    "winlogon.exe",
];

/// The game as seen by the background filter.
pub struct GameRef<'a> {
    pub pid: u32,
    pub name: &'a str,
    pub session: Option<u32>,
}

/// A process of the game's sign-in session that is not part of Windows, the game
/// (or one of its child processes, or another copy of its executable) or this app.
pub fn is_background_candidate(
    p: &ProcInfo,
    session: Option<u32>,
    game: &GameRef,
    own_pid: u32,
) -> bool {
    let name = p.name.to_lowercase();
    p.pid > 4
        && p.pid != own_pid
        && p.pid != game.pid
        && p.parent != game.pid
        && !name.eq_ignore_ascii_case(game.name)
        && session.is_some()
        && session == game.session
        && !NEVER_CALM.contains(&name.as_str())
}

struct BgChange {
    pid: u32,
    created: u64,
    priority: bool,
    throttling: Option<(u32, u32)>,
    io: Option<u32>,
}

fn calm(pid: u32) -> Option<BgChange> {
    let h = process::open(
        pid,
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION | PROCESS_SET_INFORMATION,
    )?;
    let created = process::times(&h)?.created;
    // Only apps at normal priority: a custom priority was chosen by someone.
    if unsafe { GetPriorityClass(h.0) } != NORMAL_PRIORITY_CLASS {
        return None;
    }
    let mut c = BgChange {
        pid,
        created,
        priority: unsafe { SetPriorityClass(h.0, BELOW_NORMAL_PRIORITY_CLASS) } != 0,
        throttling: None,
        io: None,
    };
    // Changed only when the old value can be read back, so it can be restored exactly.
    if let Some(prev) = get_throttling(&h) {
        let speed = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        if set_throttling(&h, speed, speed) {
            c.throttling = Some(prev);
        }
    }
    if let Some(prev) = get_io_priority(&h) {
        if prev > IO_PRIORITY_LOW && set_io_priority(&h, IO_PRIORITY_LOW) {
            c.io = Some(prev);
        }
    }
    (c.priority || c.throttling.is_some() || c.io.is_some()).then_some(c)
}

fn uncalm(c: &BgChange) {
    let Some(h) = open_same(c.pid, c.created, PROCESS_SET_INFORMATION) else {
        return;
    };
    if c.priority && unsafe { GetPriorityClass(h.0) } == BELOW_NORMAL_PRIORITY_CLASS {
        unsafe { SetPriorityClass(h.0, NORMAL_PRIORITY_CLASS) };
    }
    if let Some((control, state)) = c.throttling {
        set_throttling(&h, control, state);
    }
    if let Some(io) = c.io {
        set_io_priority(&h, io);
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// What was changed for one game, so it can be undone.
pub struct Session {
    pub pid: u32,
    created: u64,
    /// Previous priority class, when it was changed.
    priority: Option<u32>,
    /// Previous power-throttling state, when it was changed.
    throttling: Option<(u32, u32)>,
    /// Previous power plan, when it was changed.
    power_scheme: Option<GUID>,
    background: Vec<BgChange>,
    cpu_sets: bool,
    pub results: Vec<(Technique, Outcome)>,
    restored: bool,
}

/// Applies the chosen techniques to `pid`. `None` if the process is gone.
pub fn apply(pid: u32, opts: Options) -> Option<Session> {
    let created = process::creation_time(pid)?;
    let game = process::open(
        pid,
        PROCESS_QUERY_LIMITED_INFORMATION
            | PROCESS_SET_INFORMATION
            | PROCESS_SET_LIMITED_INFORMATION,
    );
    let mut s = Session {
        pid,
        created,
        priority: None,
        throttling: None,
        power_scheme: None,
        background: Vec::new(),
        cpu_sets: false,
        results: Vec::new(),
        restored: false,
    };
    // 7: set first so a cleanup started meanwhile already skips the game.
    PROTECTED.store(pid, Ordering::Release);
    s.results.push((Technique::KeepMemory, Outcome::Applied));

    if opts.high_priority {
        let outcome = match &game {
            Some(h) => match unsafe { GetPriorityClass(h.0) } {
                0 => Outcome::Failed,
                HIGH_PRIORITY_CLASS | REALTIME_PRIORITY_CLASS => Outcome::AlreadyOn,
                prev if unsafe { SetPriorityClass(h.0, HIGH_PRIORITY_CLASS) } != 0 => {
                    s.priority = Some(prev);
                    Outcome::Applied
                }
                _ => Outcome::Failed,
            },
            None => Outcome::Failed,
        };
        s.results.push((Technique::Priority, outcome));
    }

    if opts.no_power_saving {
        let mask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
        let outcome = match &game {
            Some(h) => {
                // Unknown previous state: restore lets Windows decide again.
                let prev = get_throttling(h).unwrap_or((0, 0));
                if set_throttling(h, mask, 0) {
                    s.throttling = Some(prev);
                    Outcome::Applied
                } else {
                    Outcome::Failed
                }
            }
            None => Outcome::Failed,
        };
        s.results.push((Technique::NoPowerSaving, outcome));
    }

    if opts.power_plan {
        let outcome = match active_power_scheme() {
            Some(g) if same_guid(&g, &HIGH_PERFORMANCE) || same_guid(&g, &ULTIMATE_PERFORMANCE) => {
                Outcome::AlreadyOn
            }
            Some(g) if set_power_scheme(&HIGH_PERFORMANCE) => {
                s.power_scheme = Some(g);
                Outcome::Applied
            }
            _ => Outcome::NotAvailable,
        };
        s.results.push((Technique::PowerPlan, outcome));
    }

    if opts.performance_cores {
        let outcome = match (performance_set(&cpu_sets()), &game) {
            (None, _) => Outcome::NotAvailable,
            (Some(ids), Some(h)) => {
                if unsafe { SetProcessDefaultCpuSets(h.0, ids.as_ptr(), ids.len() as u32) } != 0 {
                    s.cpu_sets = true;
                    Outcome::Applied
                } else {
                    Outcome::Failed
                }
            }
            (Some(_), None) => Outcome::Failed,
        };
        s.results.push((Technique::PerformanceCores, outcome));
    }

    if opts.calm_background {
        let procs = process::snapshot();
        let name = procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let game_ref = GameRef {
            pid,
            name: &name,
            session: process::session_of(pid),
        };
        let own = std::process::id();
        s.background = procs
            .iter()
            .filter(|p| is_background_candidate(p, process::session_of(p.pid), &game_ref, own))
            .filter_map(|p| calm(p.pid))
            .collect();
        s.results.push((
            Technique::Background,
            Outcome::Calmed(s.background.len() as u32),
        ));
    }

    if opts.free_memory {
        let before = memory::status()
            .map(|m| memory::round_kb(m.avail_phys))
            .ok();
        let trimmed = techniques::trim_all_working_sets(&|| false, &[pid]).is_ok();
        let purged = techniques::purge_standby_list() == Some(0);
        std::thread::sleep(Duration::from_secs(1));
        let after = memory::status()
            .map(|m| memory::round_kb(m.avail_phys))
            .ok();
        let outcome = match (before, after) {
            (Some(b), Some(a)) if trimmed || purged => Outcome::Freed(a - b),
            _ => Outcome::Failed,
        };
        s.results.push((Technique::FreeMemory, outcome));
    }
    Some(s)
}

impl Session {
    /// Undoes everything; safe to call more than once.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = PROTECTED.compare_exchange(self.pid, 0, Ordering::AcqRel, Ordering::Acquire);
        if let Some(g) = self.power_scheme.take() {
            set_power_scheme(&g);
        }
        for c in self.background.drain(..) {
            uncalm(&c);
        }
        let Some(h) = open_same(
            self.pid,
            self.created,
            PROCESS_SET_INFORMATION | PROCESS_SET_LIMITED_INFORMATION,
        ) else {
            return; // The game has exited: nothing left to undo on it.
        };
        if let Some(prev) = self.priority.take() {
            unsafe { SetPriorityClass(h.0, prev) };
        }
        if let Some((control, state)) = self.throttling.take() {
            set_throttling(&h, control, state);
        }
        if std::mem::take(&mut self.cpu_sets) {
            unsafe { SetProcessDefaultCpuSets(h.0, std::ptr::null(), 0) };
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: u32, name: &str) -> ProcInfo {
        ProcInfo {
            pid,
            parent,
            name: name.into(),
            threads: 1,
            title: None,
        }
    }

    #[test]
    fn background_filter_spares_windows_the_game_and_us() {
        let game = GameRef {
            pid: 100,
            name: "Game.exe",
            session: Some(1),
        };
        let ok = |p: &ProcInfo, session| is_background_candidate(p, session, &game, 50);
        assert!(ok(&proc(200, 10, "browser.exe"), Some(1)));
        assert!(!ok(&proc(200, 10, "browser.exe"), Some(0))); // a service
        assert!(!ok(&proc(200, 10, "browser.exe"), None));
        assert!(!ok(&proc(100, 10, "game.exe"), Some(1))); // the game
        assert!(!ok(&proc(201, 100, "crashpad.exe"), Some(1))); // its child
        assert!(!ok(&proc(202, 10, "GAME.EXE"), Some(1))); // another copy
        assert!(!ok(&proc(50, 10, "OpenMemoryOptimizer.exe"), Some(1)));
        assert!(!ok(&proc(203, 10, "AudioDG.exe"), Some(1)));
        assert!(!ok(&proc(204, 10, "explorer.exe"), Some(1)));
        assert!(!ok(&proc(4, 0, "System"), Some(1)));
    }

    #[test]
    fn performance_set_needs_two_kinds_of_core() {
        assert_eq!(performance_set(&[]), None);
        assert_eq!(performance_set(&[(256, 0), (257, 0)]), None);
        assert_eq!(
            performance_set(&[(256, 1), (257, 1), (258, 0), (259, 0)]),
            Some(vec![256, 257])
        );
    }

    #[test]
    fn cpu_sets_cover_every_processor() {
        let n = std::thread::available_parallelism().unwrap().get();
        assert!(cpu_sets().len() >= n);
    }

    #[test]
    fn power_plan_guids() {
        assert!(same_guid(&HIGH_PERFORMANCE, &HIGH_PERFORMANCE));
        assert!(!same_guid(&HIGH_PERFORMANCE, &ULTIMATE_PERFORMANCE));
        assert!(active_power_scheme().is_some());
    }

    #[test]
    fn boosting_own_process_protects_it_and_restores() {
        let opts = Options {
            high_priority: false,
            no_power_saving: true,
            power_plan: false,
            calm_background: false,
            free_memory: false,
            performance_cores: false,
        };
        let pid = std::process::id();
        let mut s = apply(pid, opts).unwrap();
        assert_eq!(protected(), vec![pid]);
        assert!(s
            .results
            .contains(&(Technique::NoPowerSaving, Outcome::Applied)));
        s.restore();
        assert!(protected().is_empty());
        s.restore(); // idempotent
    }
}
