//! Global hotkeys: a dedicated thread owns the registrations and message loop.

use screenit_capture::Hotkey;
use std::sync::mpsc::Sender;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

/// Registers `keys` (id = position in the slice) and sends the id of every press on `tx`.
/// Returns the ids that could not be registered (already used by another application).
pub fn spawn(keys: Vec<Hotkey>, tx: Sender<usize>, wake: impl Fn() + Send + 'static) -> Vec<usize> {
    let (res_tx, res_rx) = std::sync::mpsc::channel::<Vec<usize>>();
    std::thread::spawn(move || {
        let mut failed = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            let mut mods = MOD_NOREPEAT;
            if k.ctrl {
                mods |= MOD_CONTROL;
            }
            if k.shift {
                mods |= MOD_SHIFT;
            }
            if k.alt {
                mods |= MOD_ALT;
            }
            if k.win {
                mods |= MOD_WIN;
            }
            // SAFETY: thread-owned registration with a null window; unregistered below on exit.
            if unsafe { RegisterHotKey(std::ptr::null_mut(), i as i32, mods, k.vk) } == 0 {
                failed.push(i);
            }
        }
        let _ = res_tx.send(failed);
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: standard message loop; `msg` is a valid out-pointer.
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            if msg.message == WM_HOTKEY {
                if tx.send(msg.wParam).is_err() {
                    break;
                }
                wake();
            }
        }
        for i in 0..keys.len() {
            // SAFETY: matches the registrations above.
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), i as i32);
            }
        }
    });
    res_rx.recv().unwrap_or_default()
}
