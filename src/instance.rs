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

//! Single instance per sign-in session. Not in the original.
//!
//! The first process owns a named mutex and an auto-reset "show" event. A later
//! launch finds the mutex, signals the event so the first window comes to the
//! front, and exits instead of opening a second window and tray icon.

use std::ptr::null;
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

use crate::win::{wide, OwnedHandle};

/// Base name of the kernel objects used by the app.
pub const NAME: &str = "OpenMemoryOptimizer";

fn mutex_name(name: &str) -> String {
    format!("Local\\{name}.Instance")
}

fn event_name(name: &str) -> String {
    format!("Local\\{name}.Show")
}

/// Kernel handle that may be waited on from another thread.
struct SyncHandle(OwnedHandle);

// SAFETY: event handles may be used from any thread.
unsafe impl Send for SyncHandle {}
unsafe impl Sync for SyncHandle {}

pub enum Acquired {
    /// This is the only instance; keep the value alive for the process lifetime.
    First(Instance),
    /// Another instance is running in this session.
    AlreadyRunning,
}

pub struct Instance {
    _mutex: Option<SyncHandle>,
    show: Option<Arc<SyncHandle>>,
}

impl Instance {
    /// Requests to bring this instance to the front, sent by later launches.
    pub fn show_requests(&self) -> Option<ShowRequests> {
        self.show.clone().map(ShowRequests)
    }
}

pub struct ShowRequests(Arc<SyncHandle>);

impl ShowRequests {
    /// Blocks until another launch asks to show the window. `false` on error.
    pub fn wait(&self) -> bool {
        self.wait_ms(INFINITE)
    }

    fn wait_ms(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0 .0 .0, ms) == WAIT_OBJECT_0 }
    }
}

pub fn acquire(name: &str) -> Acquired {
    let h = unsafe { CreateMutexW(null(), 0, wide(&mutex_name(name)).as_ptr()) };
    let err = unsafe { GetLastError() };
    let mutex = match OwnedHandle::new(h) {
        Some(_) if err == ERROR_ALREADY_EXISTS => return Acquired::AlreadyRunning,
        Some(m) => Some(SyncHandle(m)),
        // Created by an instance with higher rights (e.g. elevated vs. a dev build).
        None if err == ERROR_ACCESS_DENIED => return Acquired::AlreadyRunning,
        // Any other failure must not stop the app from starting.
        None => None,
    };
    let event = unsafe { CreateEventW(null(), 0, 0, wide(&event_name(name)).as_ptr()) };
    Acquired::First(Instance {
        _mutex: mutex,
        show: OwnedHandle::new(event).map(|e| Arc::new(SyncHandle(e))),
    })
}

/// Asks the running instance to show its window. Retries briefly in case it is
/// still starting and has not created its event yet.
pub fn request_show(name: &str) -> bool {
    let event_name = wide(&event_name(name));
    for _ in 0..20 {
        let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr()) };
        if let Some(event) = OwnedHandle::new(h) {
            // This process was just launched by the user, so it may hand the
            // foreground over; otherwise Windows only flashes the taskbar button.
            unsafe { AllowSetForegroundWindow(ASFW_ANY) };
            return unsafe { SetEvent(event.0) } != 0;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(test: &str) -> String {
        format!("{NAME}.Test.{}.{test}", std::process::id())
    }

    #[test]
    fn second_acquire_sees_the_first() {
        let name = unique("second");
        let first = acquire(&name);
        assert!(matches!(first, Acquired::First(_)));
        assert!(matches!(acquire(&name), Acquired::AlreadyRunning));
        drop(first);
        assert!(matches!(acquire(&name), Acquired::First(_)));
    }

    #[test]
    fn show_request_wakes_the_first_once() {
        let name = unique("show");
        let Acquired::First(first) = acquire(&name) else {
            panic!("expected the first instance");
        };
        let requests = first.show_requests().expect("show event");
        assert!(!requests.wait_ms(0));
        assert!(request_show(&name));
        assert!(requests.wait_ms(1000));
        // Auto-reset: one request, one wake-up.
        assert!(!requests.wait_ms(0));
    }
}
