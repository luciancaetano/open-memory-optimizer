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

//! Auto-optimization: port of tmrautoTimer (0xc902a0), a TTimer with Interval=60000.

use std::time::Duration;

use crate::settings::Settings;

pub const TICK: Duration = Duration::from_secs(60);
/// Low-memory mode only evaluates every 5th tick (every 5 minutes).
pub const LOWMEM_EVERY_TICKS: i64 = 5;
/// "Only when CPU is idle" means sampled CPU usage <= 5 %.
pub const IDLE_CPU_PERCENT: i64 = 5;

#[derive(Debug, Default)]
pub struct AutoState {
    ticks: i64,
}

impl AutoState {
    /// One timer tick. `avail_mb`/`total_mb` are `Round(bytes / 1 MiB)`.
    /// `sample_cpu` is only called when the idle condition is enabled and a
    /// low-memory check is due, so its delta spans ~5 minutes like the original.
    pub fn tick(
        &mut self,
        s: &Settings,
        avail_mb: i64,
        total_mb: i64,
        sample_cpu: impl FnOnce() -> i64,
    ) -> bool {
        if !s.auto_opt {
            return false;
        }
        self.ticks += 1;
        if s.lowmem_opt {
            if self.ticks < LOWMEM_EVERY_TICKS {
                return false;
            }
            self.ticks = 0;
            let cpu = if s.idle_only { sample_cpu() } else { 0 };
            let threshold = s.low_mem_percent as f64 / 100.0;
            let free_ratio = avail_mb as f64 / total_mb as f64;
            // Written as !(a > b) on purpose: like comisd/jbe in the original, a NaN
            // ratio (total_mb == 0) must not trigger an optimization.
            #[allow(clippy::neg_cmp_op_on_partial_ord)]
            if !(threshold > free_ratio) {
                return false;
            }
            !s.idle_only || cpu <= IDLE_CPU_PERCENT
        } else if s.period_opt {
            if self.ticks < s.period_minutes {
                return false;
            }
            self.ticks = 0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Settings::default();
        f(&mut s);
        s
    }

    #[test]
    fn lowmem_checks_every_fifth_tick() {
        let s = settings(|s| s.idle_only = false);
        let mut st = AutoState::default();
        // 10 % free, threshold 30 %
        let fired: Vec<bool> = (0..10).map(|_| st.tick(&s, 100, 1000, || 0)).collect();
        assert_eq!(
            fired,
            [false, false, false, false, true, false, false, false, false, true]
        );
    }

    #[test]
    fn lowmem_respects_threshold_and_idle() {
        let mut st = AutoState::default();
        let s = Settings::default(); // idle_only = true, threshold 30 %
        let mut fire = |avail, cpu| {
            (0..4).for_each(|_| assert!(!st.tick(&s, avail, 1000, || unreachable!())));
            st.tick(&s, avail, 1000, || cpu)
        };
        assert!(!fire(400, 0)); // 40 % free
        assert!(!fire(300, 0)); // exactly 30 %: strict comparison
        assert!(fire(299, 5)); // busy 5 % counts as idle
        assert!(!fire(299, 6));
    }

    #[test]
    fn period_mode_counts_minutes() {
        let s = settings(|s| {
            s.lowmem_opt = false;
            s.period_opt = true;
            s.period_minutes = 3;
        });
        let mut st = AutoState::default();
        let fired: Vec<bool> = (0..6).map(|_| st.tick(&s, 1000, 1000, || 0)).collect();
        assert_eq!(fired, [false, false, true, false, false, true]);
    }

    #[test]
    fn disabled_does_nothing() {
        let s = settings(|s| s.auto_opt = false);
        let mut st = AutoState::default();
        assert!((0..10).all(|_| !st.tick(&s, 0, 1000, || 0)));
    }
}
