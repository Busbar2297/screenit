//! Main-window tray behaviour. Minimised windows stop receiving frames (so tray events would never
//! be processed), so "tray mode" keeps the window alive off-screen and removes its taskbar button.

use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GWL_EXSTYLE, GetClientRect, GetForegroundWindow, GetShellWindow,
    GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsZoomed, SIZE_MAXIMIZED, SIZE_RESTORED, SW_HIDE, SW_RESTORE, SW_SHOWMAXIMIZED,
    SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow,
    WINDOWPLACEMENT, WM_SIZE, WPF_RESTORETOMAXIMIZED, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Set when the window was maximized (or minimized from maximized) as it went to the tray,
/// so showing it again can fill the screen instead of coming back at the restored size.
static PARKED_ZOOMED: AtomicBool = AtomicBool::new(false);

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

/// How many `SW_RESTORE`s it takes before the window is a normal, movable window.
///
/// Minimizing a maximized window sets `WPF_RESTORETOMAXIMIZED`, so the first restore comes back
/// maximized and `SetWindowPos` cannot move it. A second restore, once it is maximized and
/// visible, returns it to its normal size.
pub fn restore_passes(iconic: bool, zoomed: bool) -> u8 {
    match (iconic, zoomed) {
        (true, true) => 2,
        (true, false) | (false, true) => 1,
        (false, false) => 0,
    }
}

/// The placement says the window is maximized, or should return to maximized when restored.
fn placement_wants_zoom(h: HWND) -> bool {
    let mut place = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `place.length` is set and `h` is a window of this process.
    if unsafe { GetWindowPlacement(h, &mut place) } == 0 {
        return false;
    }
    place.showCmd == SW_SHOWMAXIMIZED as u32 || place.flags & WPF_RESTORETOMAXIMIZED != 0
}

/// Send the main window to the tray: no taskbar button, parked off-screen but still rendering.
pub fn enter_tray() {
    let Some(h) = main_window() else { return };
    // SAFETY: valid window handle owned by this process. `IsIconic` is false once restored.
    let iconic = unsafe { IsIconic(h) != 0 };
    let zoomed = is_maximized() || placement_wants_zoom(h);
    PARKED_ZOOMED.store(zoomed, Ordering::Relaxed);
    // A minimized maximized window comes back maximized from one restore, and a maximized
    // window ignores a move. Restore until it is a normal window, then move that.
    for _ in 0..restore_passes(iconic, zoomed) {
        // SAFETY: valid window handle owned by this process.
        unsafe { ShowWindow(h, SW_RESTORE) };
    }
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

/// `--minimized` at sign-in: a normal taskbar button, not an off-screen window.
/// Does not activate, so it does not cover the desktop.
pub fn minimize_to_taskbar() {
    let Some(h) = main_window() else { return };
    // SAFETY: valid window handle owned by this process.
    unsafe { ShowWindow(h, SW_SHOWMINNOACTIVE) };
}

/// Keep a sign-in minimize on the taskbar until the user restores it.
/// Returns `(still_holding, have_seen_minimized, minimize_now)`.
pub fn startup_minimize(holding: bool, seen: bool, is_minimized: bool) -> (bool, bool, bool) {
    if !holding {
        return (false, false, false);
    }
    if is_minimized {
        (true, true, false)
    } else if seen {
        (false, false, false)
    } else {
        (true, false, true)
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

/// The parked window is the one the user is trying to use.
///
/// `saw_background` is true after the parked window has been out of the foreground once.
/// Until then a window that started active (start with Windows) stays parked, so login does
/// not pop it open. A taskbar click after that, or any move back onto a real monitor, shows it.
pub fn should_surface(
    iconic: bool,
    foreground: bool,
    on_screen: bool,
    saw_background: bool,
) -> bool {
    if on_screen {
        return true;
    }
    saw_background && (iconic || foreground)
}

/// The parked window is still the foreground window, so its taskbar button stays highlighted
/// and a click only minimizes it. Give the foreground away once, then a later click can show it.
pub fn should_yield(foreground: bool, on_screen: bool, saw_background: bool) -> bool {
    foreground && !on_screen && !saw_background
}

/// winit writes `WS_EX_APPWINDOW` back whenever it syncs styles, which puts the taskbar button
/// on a window that is parked off-screen. Put the tool-window style back when it was lost.
pub fn repair_parked_style() {
    let Some(h) = main_window() else {
        return;
    };
    // SAFETY: valid window handle owned by this process.
    let iconic = unsafe { IsIconic(h) != 0 };
    if iconic {
        return;
    }
    // SAFETY: style read on a window of this process.
    let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) };
    let tool = WS_EX_TOOLWINDOW as isize;
    let app = WS_EX_APPWINDOW as isize;
    if ex & tool != 0 && ex & app == 0 {
        return;
    }
    set_toolwindow(h, true);
    // SAFETY: valid window handle owned by this process. Shown, but not activated, so the
    // event loop keeps running while the taskbar button stays gone.
    unsafe {
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}

/// The main window is minimized.
pub fn is_iconic() -> bool {
    // SAFETY: valid window handle owned by this process.
    main_window().is_some_and(|h| unsafe { IsIconic(h) } != 0)
}

/// The main window is the foreground window.
pub fn is_foreground() -> bool {
    let Some(h) = main_window() else {
        return false;
    };
    // SAFETY: both handles are compared only; neither is dereferenced.
    unsafe { GetForegroundWindow() == h }
}

/// Drop the foreground so a parked window does not keep a highlighted taskbar button.
pub fn yield_foreground() {
    let Some(ours) = main_window() else {
        return;
    };
    // SAFETY: `GetShellWindow` returns the desktop's shell window, or null.
    let shell = unsafe { GetShellWindow() };
    if shell.is_null() || unsafe { GetForegroundWindow() } != ours {
        return;
    }
    focus_window(shell);
}

/// Bring the main window back from the tray to (x, y) with a taskbar button and focus.
/// Returns whether it was maximized when it was parked, and has been maximized again.
pub fn leave_tray(x: i32, y: i32) -> bool {
    let Some(h) = main_window() else {
        return false;
    };
    set_toolwindow(h, false);
    let zoomed = PARKED_ZOOMED.swap(false, Ordering::Relaxed);
    show_at(h, x, y, zoomed);
    apply_exe_icon();
    zoomed
}

/// Show `h` at `(x, y)`. `SetWindowPlacement` replaces the restored rect, so a window that was
/// created at the off-screen park position does not snap back there on `SW_RESTORE`.
fn show_at(h: HWND, x: i32, y: i32, zoomed: bool) {
    let mut place = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `place.length` is set and `h` is a window of this process.
    unsafe {
        if GetWindowPlacement(h, &mut place) == 0 {
            place.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        }
        let width = (place.rcNormalPosition.right - place.rcNormalPosition.left).max(200);
        let height = (place.rcNormalPosition.bottom - place.rcNormalPosition.top).max(120);
        place.flags = if zoomed { WPF_RESTORETOMAXIMIZED } else { 0 };
        place.showCmd = if zoomed {
            SW_SHOWMAXIMIZED as u32
        } else {
            SW_SHOWNORMAL as u32
        };
        place.rcNormalPosition = RECT {
            left: x,
            top: y,
            right: x.saturating_add(width),
            bottom: y.saturating_add(height),
        };
        SetWindowPlacement(h, &place);
    }
    focus_window(h);
}

/// `SetForegroundWindow` fails for a process that started with Windows. Sharing input state with
/// the current foreground thread is what makes the user's click actually focus this window.
fn focus_window(h: HWND) {
    // SAFETY: thread ids are used only for AttachThreadInput; `h` is a real window.
    unsafe {
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let me = GetCurrentThreadId();
        let attached =
            fg_thread != 0 && fg_thread != me && AttachThreadInput(fg_thread, me, 1) != 0;
        SetForegroundWindow(h);
        BringWindowToTop(h);
        if attached {
            AttachThreadInput(fg_thread, me, 0);
        }
    }
}

/// Client size in physical pixels, and whether the window is maximized.
pub fn client_state() -> Option<(i32, i32, bool)> {
    let h = main_window()?;
    let mut client = RECT::default();
    // SAFETY: valid window handle owned by this process; `client` is writable.
    if unsafe { GetClientRect(h, &mut client) } == 0 || client.right <= 0 || client.bottom <= 0 {
        return None;
    }
    // SAFETY: valid window handle owned by this process.
    let zoomed = unsafe { IsZoomed(h) != 0 };
    Some((client.right, client.bottom, zoomed))
}

/// Deliver `WM_SIZE` for the real client area. The resize event Windows sent while maximizing
/// or restoring can carry a stale size, so the GL surface keeps the previous frame.
pub fn replay_client_size() {
    let Some((w, h, zoomed)) = client_state() else {
        return;
    };
    let Some(hwnd) = main_window() else {
        return;
    };
    let lp = ((h as u32) << 16) | (w as u32 & 0xffff);
    let wp = if zoomed {
        SIZE_MAXIMIZED
    } else {
        SIZE_RESTORED
    };
    // SAFETY: `hwnd` is our window. The message is handled by this process's window procedure
    // and only reports the size `GetClientRect` just returned.
    unsafe {
        SendMessageW(hwnd, WM_SIZE, wp as usize, lp as isize);
    }
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

/// The window is maximized.
pub fn is_maximized() -> bool {
    // SAFETY: valid window handle owned by this process.
    main_window().is_some_and(|h| unsafe { IsZoomed(h) } != 0)
}

/// Largest client size, in logical points, that fits the monitor's work area (screen minus the
/// taskbar), leaving room for the title bar and borders.
pub fn max_client_pt() -> Option<(f32, f32)> {
    let h = main_window()?;
    // SAFETY: valid window handle owned by this process. Nearest monitor when the window is
    // off-screen, so a tray-parked window still gets a size.
    let monitor = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `info.cbSize` is set; `monitor` came from MonitorFromWindow.
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    // SAFETY: valid window handle owned by this process.
    let dpi = unsafe { GetDpiForWindow(h) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    if !scale.is_finite() || !(0.5..=4.0).contains(&scale) {
        return None;
    }
    let w = (info.rcWork.right - info.rcWork.left) as f32 / scale - 24.0;
    let hgt = (info.rcWork.bottom - info.rcWork.top) as f32 / scale - 56.0;
    (w.is_finite() && hgt.is_finite() && w > 200.0 && hgt > 120.0).then_some((w, hgt))
}

/// `want` shrunk so a fixed window is not taller than the screen it opened on.
pub fn fit_points(want_w: f32, want_h: f32) -> (f32, f32) {
    let Some((max_w, max_h)) = max_client_pt() else {
        return (want_w, want_h);
    };
    (
        want_w.min(max_w).max(320.0).min(max_w),
        want_h.min(max_h).max(160.0).min(max_h),
    )
}

/// If the client area is still the raw point size (a 125% display kept it as pixels), scale it.
/// A window that is already scaled, or showing a different view, is left alone.
pub fn nudge_if_unscaled(width_pt: f32, height_pt: f32) -> bool {
    let Some(h) = main_window() else {
        return false;
    };
    let mut client = RECT::default();
    // SAFETY: valid window handle owned by this process; `client` is writable.
    if unsafe { GetClientRect(h, &mut client) } == 0 {
        return false;
    }
    let dw = (client.right as f32 - width_pt).abs();
    let dh = (client.bottom as f32 - height_pt).abs();
    if dw > 48.0 || dh > 48.0 {
        return true;
    }
    set_client_size(width_pt, height_pt)
}

/// Set the main window's client area to this many logical points, scaled by the window's DPI.
/// Returns false when the window does not exist yet.
pub fn set_client_size(width_pt: f32, height_pt: f32) -> bool {
    if !width_pt.is_finite() || !height_pt.is_finite() {
        return false;
    }
    let Some(h) = main_window() else {
        return false;
    };
    // SAFETY: valid window handle owned by this process.
    let dpi = unsafe { GetDpiForWindow(h) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    if !scale.is_finite() || !(0.5..=4.0).contains(&scale) {
        return false;
    }
    let width = (width_pt * scale).round() as i32;
    let height = (height_pt * scale).round() as i32;
    let width = width.clamp(1, 8192);
    let height = height.clamp(1, 8192);
    let mut outer = RECT::default();
    let mut client = RECT::default();
    // SAFETY: valid window handle owned by this process; both RECTs are writable.
    let ok = unsafe { GetWindowRect(h, &mut outer) != 0 && GetClientRect(h, &mut client) != 0 };
    if !ok {
        return false;
    }
    if client.right == width && client.bottom == height {
        return true;
    }
    let extra_w = (outer.right - outer.left).saturating_sub(client.right);
    let extra_h = (outer.bottom - outer.top).saturating_sub(client.bottom);
    // SAFETY: valid window handle owned by this process. Size includes the frame around the client.
    unsafe {
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            0,
            0,
            width.saturating_add(extra_w),
            height.saturating_add(extra_h),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        ) != 0
    }
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

#[cfg(test)]
mod tests {
    use super::restore_passes;

    #[test]
    fn a_minimized_maximized_window_needs_two_restores_before_it_can_move() {
        assert_eq!(restore_passes(true, true), 2);
        assert_eq!(restore_passes(false, true), 1);
        assert_eq!(restore_passes(true, false), 1);
        assert_eq!(restore_passes(false, false), 0);
    }

    #[test]
    fn a_window_parked_at_startup_does_not_pop_open_until_the_user_asks() {
        assert!(!super::should_surface(false, true, false, false));
        assert!(super::should_yield(true, false, false));
        assert!(!super::should_surface(true, false, false, false));
        assert!(super::should_surface(false, true, false, true));
        assert!(super::should_surface(true, false, false, true));
        assert!(super::should_surface(false, false, true, false));
        assert!(!super::should_yield(true, true, false));
        assert!(!super::should_yield(false, false, true));
    }

    #[test]
    fn sign_in_stays_minimized_on_the_taskbar_until_the_user_restores_it() {
        assert_eq!(
            super::startup_minimize(true, false, false),
            (true, false, true)
        );
        assert_eq!(
            super::startup_minimize(true, false, true),
            (true, true, false)
        );
        assert_eq!(
            super::startup_minimize(true, true, true),
            (true, true, false)
        );
        assert_eq!(
            super::startup_minimize(true, true, false),
            (false, false, false)
        );
        assert_eq!(
            super::startup_minimize(false, false, true),
            (false, false, false)
        );
    }
}
