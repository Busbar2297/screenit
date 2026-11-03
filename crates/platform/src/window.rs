//! Main-window tray behaviour. Minimised windows stop receiving frames (so tray events would never
//! be processed), so "tray mode" keeps the window alive off-screen and removes its taskbar button.

use std::os::windows::process::CommandExt;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, SW_HIDE, SW_RESTORE, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW,
    WS_EX_TOOLWINDOW,
};

struct Find<'a> {
    title: &'a str,
    out: HWND,
}

unsafe extern "system" fn find_cb(h: HWND, lparam: isize) -> i32 {
    // SAFETY (whole fn): called by EnumWindows with a valid handle; `lparam` points at our `Find`.
    let find = unsafe { &mut *(lparam as *mut Find) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, &mut pid) };
    if pid != unsafe { GetCurrentProcessId() } {
        return 1;
    }
    let mut buf = [0u16; 128];
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), 128) };
    if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == find.title {
        find.out = h;
        return 0;
    }
    1
}

/// A window of this process with exactly this title (matched by pid so other apps with the same
/// title are never touched).
fn own_window(title: &str) -> Option<HWND> {
    let mut find = Find {
        title,
        out: std::ptr::null_mut(),
    };
    // SAFETY: `find` outlives the synchronous EnumWindows call.
    unsafe { EnumWindows(Some(find_cb), &mut find as *mut Find as isize) };
    (!find.out.is_null()).then_some(find.out)
}

fn main_window() -> Option<HWND> {
    own_window("screenIt")
}

/// The main window handle as an integer (owner for modal dialogs).
pub(crate) fn main_hwnd_raw() -> Option<isize> {
    main_window().map(|h| h as isize)
}

/// Hide one of our windows (e.g. the recording toolbar) from every screen capture, including our
/// own recorder, while it stays visible on the monitor. Returns false if it was not found.
pub fn exclude_from_capture(title: &str) -> bool {
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
    let Some(h) = own_window(title) else {
        return false;
    };
    // SAFETY: valid window handle owned by this process; documented affinity constant.
    unsafe { SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE) != 0 }
}
fn set_toolwindow(h: HWND, tool: bool) {
    // SAFETY: style read-modify-write on a window of this process, bracketed by hide/show so the
    // taskbar picks up the change.
    unsafe {
        ShowWindow(h, SW_HIDE);
        let mut ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        if tool {
            ex |= WS_EX_TOOLWINDOW as isize;
            ex &= !(WS_EX_APPWINDOW as isize);
        } else {
            ex &= !(WS_EX_TOOLWINDOW as isize);
            ex |= WS_EX_APPWINDOW as isize;
        }
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
    }
}

/// Send the main window to the tray: no taskbar button, parked off-screen but still rendering.
pub fn enter_tray() {
    let Some(h) = main_window() else { return };
    // Restore first: positioning a minimised window would only change its remembered position.
    // SAFETY: valid window handle owned by this process.
    unsafe { ShowWindow(h, SW_RESTORE) };
    set_toolwindow(h, true);
    // SAFETY: as above.
    unsafe {
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}
/// Put the executable's icon on the main window. A taskbar button created after a style change
/// (tray hide/show) does not keep an icon that was only set earlier.
pub fn apply_exe_icon() {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LoadImageW, SM_CXICON, SM_CXSMICON,
        SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
    let Some(h) = main_window() else { return };
    // SAFETY: the icon is resource 1 linked into this executable; a null result means it is absent.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        // MAKEINTRESOURCE(1): the address is the resource id, not a pointer to memory.
        let id = std::ptr::without_provenance::<u16>(1);
        let big = LoadImageW(
            module,
            id,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXICON),
            GetSystemMetrics(SM_CYICON),
            0,
        );
        let small = LoadImageW(
            module,
            id,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            0,
        );
        if !big.is_null() {
            SendMessageW(h, WM_SETICON, ICON_BIG as usize, big as isize);
        }
        if !small.is_null() {
            SendMessageW(h, WM_SETICON, ICON_SMALL as usize, small as isize);
        }
    }
}

/// Tell the shell this process is screenIt, before any window exists, so the taskbar uses our icon.
pub fn set_app_user_model_id() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id: Vec<u16> = "screenIt"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: a static wide string; the call only names this process to the shell.
    unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
}

/// Bring the main window back from the tray to (x, y) with a taskbar button and focus.
pub fn leave_tray(x: i32, y: i32) {
    let Some(h) = main_window() else { return };
    set_toolwindow(h, false);
    // SAFETY: valid window handle owned by this process.
    unsafe {
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER,
        );
        ShowWindow(h, SW_RESTORE);
        SetForegroundWindow(h);
    }
    apply_exe_icon();
}

/// Restore the main window if minimised and bring it to the foreground.
pub fn activate_main() {
    let Some(h) = main_window() else { return };
    // SAFETY: valid window handle owned by this process.
    unsafe {
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}

/// Dark caption and borders for the main window, repainted immediately (Windows only redraws
/// the frame on the next activation change otherwise).
pub fn dark_title_bar() {
    let Some(h) = main_window() else { return };
    let on: i32 = 1;
    // SAFETY: valid window handle owned by this process; `on` is a 4-byte BOOL that outlives the call.
    unsafe {
        DwmSetWindowAttribute(
            h,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &on as *const i32 as *const core::ffi::c_void,
            4,
        );
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Top-left corner of the main window in physical screen pixels.
pub fn main_position() -> Option<(i32, i32)> {
    let h = main_window()?;
    let mut r = RECT::default();
    // SAFETY: valid window handle owned by this process; `r` is a writable RECT.
    (unsafe { GetWindowRect(h, &mut r) } != 0).then_some((r.left, r.top))
}

/// Move the main window's top-left corner to (x, y) physical pixels without resizing it.
pub fn move_main(x: i32, y: i32) {
    let Some(h) = main_window() else { return };
    // SAFETY: valid window handle owned by this process.
    unsafe {
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Windows can route Print Screen to the Snipping Tool, which prevents any other app from owning
/// it. Turn that routing off for the current user so screenIt's Print Screen hotkey works.
pub fn release_print_screen_from_snipping() {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("reg")
        .args([
            "add",
            r"HKCU\Control Panel\Keyboard",
            "/v",
            "PrintScreenKeyForSnippingEnabled",
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}
