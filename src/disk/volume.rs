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

//! Drives with a letter: kind, file system, size, and whether it is an SSD
//! (`IOCTL_STORAGE_QUERY_PROPERTY`, seek penalty and TRIM descriptors).

use std::ffi::c_void;

use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, StorageDeviceTrimProperty,
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DEVICE_TRIM_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY,
    STORAGE_PROPERTY_QUERY,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::win::{wide, OwnedHandle};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fixed,
    Removable,
    Network,
    /// CD/DVD, RAM disk or unknown: not listed.
    Other,
}

impl Kind {
    pub fn from_drive_type(t: u32) -> Kind {
        match t {
            DRIVE_FIXED => Kind::Fixed,
            DRIVE_REMOVABLE => Kind::Removable,
            DRIVE_REMOTE => Kind::Network,
            _ => Kind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Ssd,
    Hdd,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Upper-case drive letter.
    pub letter: char,
    pub label: String,
    /// File system name, e.g. "NTFS".
    pub fs: String,
    pub kind: Kind,
    pub media: Media,
    pub total: u64,
    pub free: u64,
    /// Windows is installed on it.
    pub system: bool,
}

impl Volume {
    /// "C:\".
    pub fn root(&self) -> String {
        format!("{}:\\", self.letter)
    }

    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Junk cleanup: local drives only (the Recycle Bin of a network share is not ours).
    pub fn can_clean(&self) -> bool {
        matches!(self.kind, Kind::Fixed | Kind::Removable)
    }

    /// The Windows optimizer handles local drives only.
    pub fn can_optimize(&self) -> bool {
        matches!(self.kind, Kind::Fixed | Kind::Removable)
    }
}

/// Letters of the bits set in a `GetLogicalDrives` mask.
pub fn letters(mask: u32) -> Vec<char> {
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i) as char)
        .collect()
}

/// The drive Windows runs from, from `%SystemDrive%` ("C:").
pub fn system_letter() -> char {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|s| s.chars().next())
        .map_or('C', |c| c.to_ascii_uppercase())
}

/// All drives with a letter except CD/DVD and RAM disks, A-Z order. Drives without
/// media (empty card readers) are left out.
pub fn list() -> Vec<Volume> {
    let system = system_letter();
    letters(unsafe { GetLogicalDrives() })
        .into_iter()
        .filter_map(|letter| describe(letter, system))
        .collect()
}

/// Free bytes on drive `letter` right now.
pub fn free_space(letter: char) -> Option<u64> {
    let root = wide(&format!("{letter}:\\"));
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    (unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0)
        .then_some(free)
}

fn describe(letter: char, system: char) -> Option<Volume> {
    let root = wide(&format!("{letter}:\\"));
    let kind = Kind::from_drive_type(unsafe { GetDriveTypeW(root.as_ptr()) });
    if kind == Kind::Other {
        return None;
    }
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return None;
    }
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } != 0;
    let text = |b: &[u16]| {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf16_lossy(&b[..end])
    };
    let media = if kind == Kind::Network {
        Media::Unknown
    } else {
        media_of(letter).0
    };
    Some(Volume {
        letter,
        label: if ok { text(&label) } else { String::new() },
        fs: if ok { text(&fs) } else { String::new() },
        kind,
        media,
        total,
        free,
        system: letter == system,
    })
}

/// SSD or hard disk, and whether the device supports TRIM. A volume handle opened
/// with no access rights is enough for these queries.
pub fn media_of(letter: char) -> (Media, Option<bool>) {
    let path = wide(&format!("\\\\.\\{letter}:"));
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    let Some(h) = OwnedHandle::new(h) else {
        return (Media::Unknown, None);
    };
    let seek: Option<DEVICE_SEEK_PENALTY_DESCRIPTOR> =
        query_property(&h, StorageDeviceSeekPenaltyProperty);
    let trim: Option<DEVICE_TRIM_DESCRIPTOR> = query_property(&h, StorageDeviceTrimProperty);
    let media = match seek {
        Some(d) if d.IncursSeekPenalty != 0 => Media::Hdd,
        Some(_) => Media::Ssd,
        None => Media::Unknown,
    };
    (media, trim.map(|d| d.TrimEnabled != 0))
}

fn query_property<T: Copy>(h: &OwnedHandle, id: i32) -> Option<T> {
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: id,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut out: T = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const c_void,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            &mut out as *mut T as *mut c_void,
            std::mem::size_of::<T>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    } != 0;
    (ok && returned as usize >= std::mem::size_of::<T>()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_to_letters() {
        assert_eq!(letters(0b1101), vec!['A', 'C', 'D']);
        assert_eq!(letters(1 << 25), vec!['Z']);
        assert!(letters(0).is_empty());
    }

    #[test]
    fn drive_types() {
        assert_eq!(Kind::from_drive_type(3), Kind::Fixed);
        assert_eq!(Kind::from_drive_type(2), Kind::Removable);
        assert_eq!(Kind::from_drive_type(4), Kind::Network);
        assert_eq!(Kind::from_drive_type(5), Kind::Other); // CD-ROM
    }

    #[test]
    fn network_drives_are_analyze_only() {
        let v = Volume {
            letter: 'Z',
            label: String::new(),
            fs: "NTFS".into(),
            kind: Kind::Network,
            media: Media::Unknown,
            total: 10,
            free: 4,
            system: false,
        };
        assert!(!v.can_clean() && !v.can_optimize());
        assert_eq!((v.root().as_str(), v.used()), ("Z:\\", 6));
    }

    #[test]
    fn lists_the_system_drive() {
        let all = list();
        assert!(all.iter().any(|v| v.system && v.total > 0));
    }
}
