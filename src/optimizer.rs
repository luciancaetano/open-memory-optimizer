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

//! One optimization pass: port of the worker thread at 0xc8d930.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::memory;
use crate::techniques::{self, DefragStats, FileCacheReport, TrimStats};

/// The worker's second argument: 0 from the Optimize button, 1 from the auto timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Runs every selected method and lasts at least 3 seconds.
    Manual,
    /// Skips defrag and standby purge; always trims working sets.
    Auto,
}

#[derive(Debug, Clone, Copy)]
pub struct Methods {
    pub clear_clipboard: bool,
    pub defrag: bool,
    pub trim_working_sets: bool,
    pub purge_standby: bool,
}

impl Methods {
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            clear_clipboard: s.clear_clipboard,
            defrag: s.defrag_mem,
            trim_working_sets: s.trim_working_sets,
            purge_standby: s.clear_standby,
        }
    }

    /// A manual run needs at least one of these (0xc8d33d).
    pub fn any_selected(&self) -> bool {
        self.defrag || self.trim_working_sets || self.purge_standby
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub clipboard_cleared: Option<bool>,
    pub defrag: Option<DefragStats>,
    pub trim: Option<std::io::Result<TrimStats>>,
    pub standby_status: Option<Option<i32>>,
    pub file_cache: Option<Result<FileCacheReport, &'static str>>,
    pub before_kb: i64,
    pub after_kb: i64,
    pub elapsed: Duration,
    /// Stopped early by [`cancel`]; later steps were skipped.
    pub cancelled: bool,
}

impl Report {
    /// Honest difference in available memory. The original instead reports a
    /// random 1-10 MB when this is not positive (0xc8daf6: (Random(10)+1) shl 10).
    pub fn freed_kb(&self) -> i64 {
        self.after_kb - self.before_kb
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Asks the running pass to stop at the next safe point: between steps, between
/// processes while trimming, or between blocks while deep cleaning. Memory that
/// deep cleaning already took is still given back. Does nothing when idle.
/// Not in the original, which cannot be stopped.
pub fn cancel() {
    if RUNNING.load(Ordering::Acquire) {
        CANCEL.store(true, Ordering::Release);
    }
}

fn cancelled() -> bool {
    CANCEL.load(Ordering::Acquire)
}

fn avail_kb() -> i64 {
    memory::status()
        .map(|s| memory::round_kb(s.avail_phys))
        .unwrap_or(0)
}

/// Returns `None` if a pass is already running.
pub fn run(mode: Mode, methods: Methods, log: &mut dyn FnMut(&str)) -> Option<Report> {
    if RUNNING.swap(true, Ordering::Acquire) {
        return None;
    }
    CANCEL.store(false, Ordering::Release);
    let start = Instant::now();
    let mut r = Report::default();

    if methods.clear_clipboard && !cancelled() {
        log("Clearing clipboard");
        r.clipboard_cleared = Some(techniques::clear_clipboard());
    }

    let before = memory::status().ok();
    r.before_kb = before.map(|s| memory::round_kb(s.avail_phys)).unwrap_or(0);

    if mode == Mode::Manual && methods.defrag && !cancelled() {
        let blocks = techniques::defrag_block_count(before.map(|s| s.avail_phys).unwrap_or(0));
        log(&format!("Defragmenting memory: committing {blocks} x 1 MB"));
        r.defrag = techniques::defrag_memory(blocks, &cancelled);
    }
    if (mode == Mode::Auto || methods.trim_working_sets) && !cancelled() {
        log("Trimming working sets of all processes");
        r.trim = Some(techniques::trim_all_working_sets(
            &cancelled,
            &crate::booster::protected(),
        ));
    }
    if mode == Mode::Manual && methods.purge_standby && !cancelled() {
        log("Purging standby list");
        r.standby_status = Some(techniques::purge_standby_list());
    }
    if !cancelled() {
        log("Trimming system file cache");
        r.file_cache = Some(techniques::trim_file_cache());
    }

    // Let the memory manager settle: 3 x Sleep(1000), in short slices so Stop
    // answers quickly.
    for _ in 0..30 {
        if cancelled() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    r.after_kb = avail_kb();

    // Manual runs are padded to a minimum of 3 s (always true after the sleeps above).
    if mode == Mode::Manual && !cancelled() {
        let floor = Duration::from_millis(3000);
        if let Some(rest) = floor.checked_sub(start.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    r.elapsed = start.elapsed();
    r.cancelled = cancelled();
    CANCEL.store(false, Ordering::Release);
    RUNNING.store(false, Ordering::Release);
    Some(r)
}
