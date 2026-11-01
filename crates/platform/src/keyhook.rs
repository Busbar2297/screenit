//! System-wide Print Screen. `RegisterHotKey` fails while another capture tool (Snagit, Snipping
//! Tool, ...) owns the key, so two independent listeners run on one thread:
//! - a `WH_KEYBOARD_LL` hook, which also swallows the key so the other tool does not fire too;
//! - Raw Input (`RIDEV_INPUTSINK`) on a message-only window, which still sees the key when other
//!   applications' hooks keep the low-level hook from being called.
//!
//! Both report the same press, so presses closer together than `DEBOUNCE_MS` count once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK,
    RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HC_ACTION,
    HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, RegisterClassW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_INPUT, WM_KEYDOWN, WM_SYSKEYDOWN, WNDCLASSW,
};

type Callback = Box<dyn Fn() + Send + 'static>;

static ON_PRINT: OnceLock<Mutex<Option<Callback>>> = OnceLock::new();
static LAST_FIRE_MS: AtomicU64 = AtomicU64::new(0);
const DEBOUNCE_MS: u64 = 600;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn held(vk: u16) -> bool {
    // SAFETY: plain state query, no pointers.
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

fn no_modifiers() -> bool {
    ![VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(held)
}

/// Fire the callback unless the other listener already reported this press.
fn fire() -> bool {
    let now = now_ms();
    let last = LAST_FIRE_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < DEBOUNCE_MS {
        return true;
    }
    LAST_FIRE_MS.store(now, Ordering::Relaxed);
    match ON_PRINT.get().map(|m| m.lock()) {
        Some(Ok(cb)) => match cb.as_ref() {
            Some(cb) => {
                cb();
                true
            }
            None => false,
        },
        _ => false,
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
        // SAFETY: for HC_ACTION, `lparam` points at a KBDLLHOOKSTRUCT owned by the system.
        let vk = unsafe { (*(lparam as *const KBDLLHOOKSTRUCT)).vkCode };
        if vk == VK_SNAPSHOT as u32 && no_modifiers() && fire() {
            return 1;
        }
    }
    // SAFETY: forwarding the unmodified hook arguments.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        // SAFETY: `raw` is a writable RAWINPUT and `size` holds its capacity.
        let n = unsafe {
            GetRawInputData(
                lparam as _,
                RID_INPUT,
                &mut raw as *mut RAWINPUT as *mut _,
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if n != u32::MAX && raw.header.dwType == RIM_TYPEKEYBOARD {
            // SAFETY: header says this is keyboard data.
            let kb = unsafe { raw.data.keyboard };
            // Print Screen often arrives as a release only, so either edge counts (debounced).
            if kb.VKey == VK_SNAPSHOT && no_modifiers() {
                fire();
            }
        }
    }
    // SAFETY: default processing for every message (required after WM_INPUT too).
    unsafe { DefWindowProcW(h, msg, wparam, lparam) }
}

/// Message-only window registered for background keyboard Raw Input.
fn raw_input_window() -> bool {
    let class: Vec<u16> = "screenItRawInput\0".encode_utf16().collect();
    // SAFETY: class/window creation with static strings and our own window procedure.
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return false;
        }
        let dev = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        RegisterRawInputDevices(&dev, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) != 0
    }
}

/// Call `on_print` on every plain Print Screen press (no modifiers), system-wide.
/// Returns false when neither listener could be installed.
pub fn spawn_print_screen_hook(on_print: impl Fn() + Send + 'static) -> bool {
    let slot = ON_PRINT.get_or_init(|| Mutex::new(None));
    if let Ok(mut s) = slot.lock() {
        *s = Some(Box::new(on_print));
    }
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || {
        // SAFETY: the hook procedure lives for the whole program; the module handle is our exe.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(hook_proc),
                GetModuleHandleW(std::ptr::null()),
                0,
            )
        };
        let raw = raw_input_window();
        let _ = tx.send(!hook.is_null() || raw);
        // SAFETY: zeroed MSG is a valid out-parameter for GetMessageW.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        // Both the low-level hook and Raw Input need a message loop on this thread.
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            // SAFETY: `msg` was filled by GetMessageW.
            unsafe { DispatchMessageW(&msg) };
        }
        if !hook.is_null() {
            // SAFETY: matches SetWindowsHookExW above.
            unsafe { UnhookWindowsHookEx(hook) };
        }
    });
    rx.recv().unwrap_or(false)
}
