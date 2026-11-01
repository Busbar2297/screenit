//! Single running instance. A named mutex marks the first process; a later launch signals a named
//! event that the first process waits on (so it can bring its window forward) and then exits.

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

const MUTEX: &str = "Local\\screenIt.SingleInstance";
const EVENT: &str = "Local\\screenIt.Activate";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub enum Instance {
    /// This is the only instance; keep the handle to hear about later launches.
    First(Activation),
    /// Another instance is running and has been asked to show itself.
    Existing,
}

/// The "show yourself" event (raw handle; 0 when it could not be created).
pub struct Activation(isize);

/// Claim the single-instance slot, or signal the instance that already holds it.
pub fn acquire() -> Instance {
    let (ev_name, mx_name) = (wide(EVENT), wide(MUTEX));
    // SAFETY: names are NUL-terminated and outlive the calls; handles are checked before use and
    // the mutex handle is deliberately kept open for the life of the first process.
    unsafe {
        // The event is created (or opened) before the mutex, so a second launch always finds it.
        let ev = CreateEventW(std::ptr::null(), 0, 0, ev_name.as_ptr());
        let mx = CreateMutexW(std::ptr::null(), 0, mx_name.as_ptr());
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        if mx.is_null() || !existed {
            return Instance::First(Activation(ev as isize));
        }
        // Let the running instance take the foreground: this process got it from the launch.
        AllowSetForegroundWindow(ASFW_ANY);
        if !ev.is_null() {
            SetEvent(ev);
            CloseHandle(ev);
        }
        CloseHandle(mx);
        Instance::Existing
    }
}

impl Activation {
    /// Call `f` on a background thread each time another launch asks this instance to show itself.
    pub fn on_activate(self, f: impl Fn() + Send + 'static) {
        let h = self.0;
        if h == 0 {
            return;
        }
        std::thread::spawn(move || {
            // SAFETY: `h` is an event handle owned by this process and never closed.
            while unsafe { WaitForSingleObject(h as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
                f();
            }
        });
    }
}
