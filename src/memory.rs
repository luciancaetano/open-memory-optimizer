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

//! Memory measurements: GlobalMemoryStatusEx (used for the before/after numbers
//! and the low-memory trigger) and the PDH page-list counters the original
//! shows in its detail view (TWindowsMemoryInfo2 at 0xc8a5xx).

use std::mem::{size_of, zeroed};

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::win::wide;

#[derive(Debug, Clone, Copy)]
pub struct MemStatus {
    pub load_percent: u32,
    pub total_phys: u64,
    pub avail_phys: u64,
    pub total_page_file: u64,
    pub avail_page_file: u64,
}

pub fn status() -> std::io::Result<MemStatus> {
    let mut ms: MEMORYSTATUSEX = unsafe { zeroed() };
    ms.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(MemStatus {
        load_percent: ms.dwMemoryLoad,
        total_phys: ms.ullTotalPhys,
        avail_phys: ms.ullAvailPhys,
        total_page_file: ms.ullTotalPageFile,
        avail_page_file: ms.ullAvailPageFile,
    })
}

/// Delphi `Round` (cvtsd2si, round-half-to-even) of `bytes / 1024`.
pub fn round_kb(bytes: u64) -> i64 {
    (bytes as f64 / 1024.0).round_ties_even() as i64
}

/// Delphi `Round` of `bytes / 1048576`.
pub fn round_mb(bytes: u64) -> i64 {
    (bytes as f64 / 1_048_576.0).round_ties_even() as i64
}

/// Counter paths copied from the original binary.
pub const PAGE_LIST_COUNTERS: [(&str, &str); 4] = [
    ("Modified page list", r"\Memory\Modified Page List Bytes"),
    (
        "Standby (normal priority)",
        r"\Memory\Standby Cache Normal Priority Bytes",
    ),
    ("Standby (core)", r"\Memory\Standby Cache Core Bytes"),
    ("Standby (reserve)", r"\Memory\Standby Cache Reserve Bytes"),
];

/// Reads the page-list gauges. English counter names are used so this also
/// works on localized Windows (the original's choice here is not established).
pub fn page_lists() -> Vec<(&'static str, Option<u64>)> {
    unsafe {
        let mut query = zeroed();
        if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
            return PAGE_LIST_COUNTERS.iter().map(|(n, _)| (*n, None)).collect();
        }
        let counters: Vec<_> = PAGE_LIST_COUNTERS
            .iter()
            .map(|(label, path)| {
                let path = wide(path);
                let mut counter = zeroed();
                let ok = PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) == 0;
                (*label, ok.then_some(counter))
            })
            .collect();
        let collected = PdhCollectQueryData(query) == 0;
        let result = counters
            .into_iter()
            .map(|(label, counter)| {
                let value = counter.filter(|_| collected).and_then(|c| {
                    let mut v: PDH_FMT_COUNTERVALUE = zeroed();
                    (PdhGetFormattedCounterValue(c, PDH_FMT_LARGE, std::ptr::null_mut(), &mut v)
                        == 0)
                        .then_some(v.Anonymous.largeValue as u64)
                });
                (label, value)
            })
            .collect();
        PdhCloseQuery(query);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_cvtsd2si() {
        assert_eq!(round_kb(1536), 2); // 1.5 -> 2 (ties to even)
        assert_eq!(round_kb(2560), 2); // 2.5 -> 2 (ties to even)
        assert_eq!(round_mb(3 * 1_048_576 + 524_288), 4); // 3.5 -> 4
    }

    #[test]
    fn status_is_sane() {
        let s = status().unwrap();
        assert!(s.total_phys > 0 && s.avail_phys <= s.total_phys);
    }
}
