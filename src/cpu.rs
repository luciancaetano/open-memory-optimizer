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

//! CPU usage sampler, port of 0xc90190 (used by the "only when CPU is idle" option).
//!
//! The original queries three NtQuerySystemInformation classes with fixed buffer
//! sizes and derives busy % from the change in idle-process time between calls:
//!   class 3 SystemTimeOfDayInformation  (0x20 bytes, CurrentTime at +0x08)
//!   class 2 SystemPerformanceInformation (0x138 bytes, IdleProcessTime at +0x00)
//!   class 0 SystemBasicInformation      (0x40 bytes, NumberOfProcessors at +0x38)
//!   cpu = Trunc(100 - (dIdle / dTime) * 100 / NumberOfProcessors)
//! The first call (no previous sample) and any query failure return 0.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::win::resolve;

type NtQuerySystemInformation =
    unsafe extern "system" fn(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;

fn nt_query() -> Option<NtQuerySystemInformation> {
    static F: OnceLock<Option<usize>> = OnceLock::new();
    F.get_or_init(|| resolve("ntdll.dll", "NtQuerySystemInformation").map(|p| p as usize))
        .map(|p| unsafe { std::mem::transmute::<usize, NtQuerySystemInformation>(p) })
}

fn query<const N: usize>(f: NtQuerySystemInformation, class: u32, len: u32) -> Option<[u64; N]> {
    let mut buf = [0u64; N];
    let status = unsafe { f(class, buf.as_mut_ptr().cast(), len, std::ptr::null_mut()) };
    (status == 0).then_some(buf)
}

#[derive(Debug, Default)]
pub struct CpuSampler {
    prev_idle: i64,
    prev_time: i64,
}

impl CpuSampler {
    pub fn sample(&mut self) -> i64 {
        let Some(f) = nt_query() else { return 0 };
        let Some(tod) = query::<4>(f, 3, 0x20) else {
            return 0;
        };
        let Some(perf) = query::<39>(f, 2, 0x138) else {
            return 0;
        };
        let Some(basic) = query::<8>(f, 0, 0x40) else {
            return 0;
        };
        let now = tod[1] as i64;
        let idle = perf[0] as i64;
        let nprocs = basic[7].to_le_bytes()[0];
        self.advance(idle, now, nprocs)
    }

    /// Pure part of the computation, kept separate for testing.
    fn advance(&mut self, idle: i64, now: i64, nprocs: u8) -> i64 {
        let mut cpu = 0;
        if self.prev_idle != 0 && nprocs != 0 {
            let d_idle = idle - self.prev_idle;
            let d_time = now - self.prev_time;
            if d_time != 0 {
                let idle_pct = d_idle as f64 / d_time as f64 * 100.0 / nprocs as f64;
                cpu = (100.0 - idle_pct).trunc() as i64;
            }
        }
        self.prev_idle = idle;
        self.prev_time = now;
        cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_is_zero_then_delta() {
        let mut s = CpuSampler::default();
        assert_eq!(s.advance(1_000, 10_000, 4), 0);
        // 4 CPUs, 10_000 ticks elapsed, 30_000 idle ticks => 75% idle => 25% busy
        assert_eq!(s.advance(31_000, 20_000, 4), 25);
        // truncation, not rounding: 33.33..% idle -> 66.66 -> 66
        assert_eq!(s.advance(31_000 + 13_333, 30_000, 4), 66);
    }

    #[test]
    fn live_sample_in_range() {
        let mut s = CpuSampler::default();
        s.sample();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let v = s.sample();
        assert!((-5..=100).contains(&v), "cpu={v}");
    }
}
