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

//! Opens a folder in File Explorer, the only action the analyzer offers.

use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize};
use windows_sys::Win32::UI::Shell::{
    ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::win::wide;

const COINIT_APARTMENTTHREADED: u32 = 0x2;

/// Opens `folder` in File Explorer. Runs on its own thread so the window never waits.
pub fn open_folder(folder: &str) {
    let folder = folder.to_owned();
    std::thread::spawn(move || {
        let path = wide(&folder);
        let verb = wide("explore");
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    });
}

/// Opens the folder that holds `file`, with the file selected.
pub fn show_file(file: &str) {
    let file = file.to_owned();
    std::thread::spawn(move || unsafe {
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED) >= 0;
        let path = wide(&file);
        let pidl = ILCreateFromPathW(path.as_ptr());
        if !pidl.is_null() {
            SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0);
            ILFree(pidl);
        }
        if com {
            CoUninitialize();
        }
    });
}
