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

//! Disk module: analyze what fills a drive, clean up junk files and optimize the drive.
//! Not in the original, which only frees memory.
//!
//! - [`volume`] lists the drives and tells SSDs from hard disks.
//! - [`scan`] walks a drive and finds the biggest folders and files. It never deletes.
//! - [`clean`] measures and removes junk with strict safety rules; the Windows parts
//!   go through the Disk Cleanup handlers in [`volcache`].
//! - [`optimize`] runs the Windows tools (defrag, fsutil, powercfg, DISM).
//! - [`reveal`] opens a folder in File Explorer.

pub mod clean;
pub mod optimize;
pub mod reveal;
pub mod scan;
pub mod volcache;
pub mod volume;

/// Bytes as "512 KB", "2.00 MB" or "3.00 GB" (via `win::format_kb`), without the
/// language's decimal separator; the GUI uses `Strings::size`.
pub fn kb(bytes: u64) -> i64 {
    (bytes / 1024) as i64
}

/// `100 * part / whole`, 0 when `whole` is 0.
pub fn percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 * 100.0 / whole as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_handles_zero() {
        assert_eq!(percent(5, 0), 0.0);
        assert_eq!(percent(1, 4), 25.0);
        assert_eq!(kb(4096), 4);
    }
}
