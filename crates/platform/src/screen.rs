//! Monitor/window enumeration and pixel grabbing via GDI + DWM.

use crate::util::{from_wide, hwnd_from_id, id_from_hwnd};
use screenit_capture::{CaptureError, CursorInfo, MonitorInfo, ScreenSource, WindowInfo};
use screenit_geom::{Point, Rect};
use screenit_raster::Raster;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Storage::Xps::PrintWindow;

type WinBool = i32;
const TRUE: WinBool = 1;
const MONITORINFOF_PRIMARY: u32 = 1;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC,
    GetDIBits, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW, ReleaseDC, SRCCOPY, ScreenToClient,
    SelectObject,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, ChildWindowFromPointEx, GetClassNameW, SetCursorPos,
    WindowFromPoint,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_TOOLWINDOW,
};

fn platform<E: std::fmt::Display>(e: E) -> CaptureError {
    CaptureError::Platform(e.to_string())
}

#[derive(Default)]
pub struct Win32Screen {
    /// GPU desktop duplication used while recording; GDI is the fallback.
    dup: std::sync::Mutex<crate::dup::Duplicator>,
}

impl Win32Screen {
    pub fn new() -> Self {
        Self::default()
    }
}

fn rect_of(r: RECT) -> Rect {
    Rect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

unsafe extern "system" fn monitor_cb(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> WinBool {
    // SAFETY: `data` is the `&mut Vec<MonitorInfo>` passed by `monitors()` below.
    let out = unsafe { &mut *(data as *mut Vec<MonitorInfo>) };
    let mut mi: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `mi` is a properly sized, zero-initialised MONITORINFOEXW.
    if unsafe { GetMonitorInfoW(m, &mut mi as *mut MONITORINFOEXW as *mut _) } != 0 {
        out.push(MonitorInfo {
            id: out.len() as u32,
            name: from_wide(&mi.szDevice),
            rect: rect_of(mi.monitorInfo.rcMonitor),
            // SAFETY: no arguments.
            dpi: unsafe { GetDpiForSystem() },
            primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    TRUE
}

unsafe extern "system" fn menu_cb(h: HWND, data: LPARAM) -> WinBool {
    // SAFETY: data is the &mut Vec<WindowInfo> passed by menus().
    let out = unsafe { &mut *(data as *mut Vec<WindowInfo>) };
    let mut name = [0u16; 32];
    // SAFETY: valid buffer and handle.
    let n = unsafe { GetClassNameW(h, name.as_mut_ptr(), name.len() as i32) };
    if n > 0 && from_wide(&name[..n as usize]) == "#32768" && unsafe { IsWindowVisible(h) } != 0 {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(h, &mut r) } != 0 {
            out.push(WindowInfo {
                id: id_from_hwnd(h),
                title: "Menu".into(),
                process: String::new(),
                rect: rect_of(r),
                minimized: false,
            });
        }
    }
    TRUE
}

unsafe extern "system" fn window_cb(h: HWND, data: LPARAM) -> WinBool {
    // SAFETY: `data` is the `&mut Vec<WindowInfo>` passed by `windows()` below.
    let out = unsafe { &mut *(data as *mut Vec<WindowInfo>) };
    if let Some(w) = window_info(h) {
        out.push(w);
    }
    TRUE
}

/// Frame bounds without the invisible resize border Windows 10/11 adds.
fn visible_bounds(h: HWND) -> Option<Rect> {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    // SAFETY: out-pointer is a valid RECT and the size matches.
    let hr = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if hr == 0 {
        return Some(rect_of(r));
    }
    // SAFETY: valid RECT out-pointer.
    (unsafe { GetWindowRect(h, &mut r) } != 0).then(|| rect_of(r))
}

fn window_info(h: HWND) -> Option<WindowInfo> {
    // SAFETY: all calls below take a window handle and plain out-pointers.
    unsafe {
        if IsWindowVisible(h) == 0 {
            return None;
        }
        if GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut c_void,
            4,
        );
        if cloaked != 0 {
            return None;
        }
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        let title = from_wide(&buf[..n.max(0) as usize]);
        let rect = visible_bounds(h)?;
        if rect.is_empty() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        Some(WindowInfo {
            id: id_from_hwnd(h),
            title,
            process: pid.to_string(),
            rect,
            minimized: IsIconic(h) != 0,
        })
    }
}

/// Copy `rect` of the screen DC (or an arbitrary source via `fill`) into a Raster.
fn grab_dc(w: u32, h: u32, fill: impl FnOnce(HDC) -> bool) -> Result<Raster, CaptureError> {
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    // SAFETY: standard GDI sequence; every created object is released on every path below.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(platform("GetDC failed"));
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let old = SelectObject(mem, bmp);
        let ok = !mem.is_null() && !bmp.is_null() && fill(mem);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        let lines = if ok {
            GetDIBits(
                mem,
                bmp,
                0,
                h,
                buf.as_mut_ptr() as *mut c_void,
                &mut bi,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if !ok || lines as u32 != h {
            return Err(platform("screen copy failed"));
        }
        Raster::from_bgra(w, h, w as usize * 4, &buf, true).map_err(CaptureError::from)
    }
}

impl ScreenSource for Win32Screen {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        let mut out: Vec<MonitorInfo> = Vec::new();
        // SAFETY: the callback only dereferences the Vec pointer we pass, which outlives the call.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_cb),
                &mut out as *mut Vec<MonitorInfo> as LPARAM,
            );
        }
        if out.is_empty() {
            Err(CaptureError::Empty)
        } else {
            Ok(out)
        }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        let mut out: Vec<WindowInfo> = Vec::new();
        // SAFETY: as above; EnumWindows walks top-level windows in z-order (topmost first).
        unsafe {
            EnumWindows(Some(window_cb), &mut out as *mut Vec<WindowInfo> as LPARAM);
        }
        Ok(out)
    }

    fn grab(&self, rect: Rect) -> Result<Raster, CaptureError> {
        let (x, y) = (rect.x, rect.y);
        grab_dc(rect.w, rect.h, |mem| {
            // SAFETY: `mem` is a valid memory DC; the screen DC is acquired and released here.
            unsafe {
                let screen = GetDC(std::ptr::null_mut());
                let ok = BitBlt(
                    mem,
                    0,
                    0,
                    rect.w as i32,
                    rect.h as i32,
                    screen,
                    x,
                    y,
                    SRCCOPY | CAPTUREBLT,
                );
                ReleaseDC(std::ptr::null_mut(), screen);
                ok != 0
            }
        })
    }

    fn grab_stream(&self, rect: Rect) -> Result<Raster, CaptureError> {
        let fast = self
            .dup
            .lock()
            .ok()
            .and_then(|mut d| d.grab(rect, &mut |r| self.grab(r).ok()));
        if let Some(r) = fast {
            return Ok(r);
        }
        self.grab(rect)
    }

    fn grab_window(&self, id: u64) -> Result<Raster, CaptureError> {
        let h = hwnd_from_id(id);
        let info = window_info(h).ok_or(CaptureError::NoWindow(id))?;
        // Full window rect (with invisible borders) for PrintWindow, then crop to visible bounds.
        let mut full: RECT = unsafe { std::mem::zeroed() };
        // SAFETY: valid RECT out-pointer.
        if unsafe { GetWindowRect(h, &mut full) } == 0 {
            return Err(CaptureError::NoWindow(id));
        }
        let full = rect_of(full);
        let printed = grab_dc(full.w, full.h, |mem| {
            // PW_RENDERFULLCONTENT (2) captures DWM-composed content including layered windows.
            // SAFETY: valid window and memory DC.
            unsafe { PrintWindow(h, mem, 2) != 0 }
        });
        match printed {
            Ok(img) => {
                let vis = Rect::new(
                    info.rect.x - full.x,
                    info.rect.y - full.y,
                    info.rect.w,
                    info.rect.h,
                );
                img.crop(vis).ok_or(CaptureError::Empty)
            }
            Err(_) => self.grab(info.rect),
        }
    }

    fn cursor(&self) -> Option<CursorInfo> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: valid POINT out-pointer.
        (unsafe { GetCursorPos(&mut p) } != 0).then(|| CursorInfo {
            pos: Point::new(p.x, p.y),
        })
    }

    fn scroll(&self, at: Point, notches: i32) -> Result<(), CaptureError> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
        };
        // SAFETY: move the cursor over the target, then inject one wheel event; INPUT is POD.
        unsafe {
            SetCursorPos(at.x, at.y);
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: (-(notches.clamp(-50, 50)) * 120) as u32,
                        dwFlags: MOUSEEVENTF_WHEEL,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            if SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) != 1 {
                return Err(platform("SendInput failed"));
            }
        }
        Ok(())
    }

    fn secret_fields(&self, rect: Rect) -> Vec<Rect> {
        let ids: Vec<u64> = self
            .windows()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| !w.minimized && w.rect.intersect(&rect).is_some())
            .map(|w| w.id)
            .collect();
        crate::uia::password_fields(ids)
            .into_iter()
            .filter(|r| r.intersect(&rect).is_some())
            .collect()
    }

    fn object_at(&self, p: Point) -> Option<WindowInfo> {
        // SAFETY: plain window-handle queries.
        unsafe {
            let mut h = WindowFromPoint(POINT { x: p.x, y: p.y });
            if h.is_null() {
                return None;
            }
            // Descend to the deepest child that contains the point.
            loop {
                let mut pt = POINT { x: p.x, y: p.y };
                ScreenToClient(h, &mut pt);
                let child = ChildWindowFromPointEx(h, pt, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT);
                if child.is_null() || child == h {
                    break;
                }
                h = child;
            }
            let mut r: RECT = std::mem::zeroed();
            if GetWindowRect(h, &mut r) == 0 {
                return None;
            }
            let mut buf = [0u16; 128];
            let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
            Some(WindowInfo {
                id: id_from_hwnd(h),
                title: from_wide(&buf[..n.max(0) as usize]),
                process: String::new(),
                rect: rect_of(r),
                minimized: false,
            })
        }
    }

    fn menus(&self) -> Vec<WindowInfo> {
        let mut out: Vec<WindowInfo> = Vec::new();
        // SAFETY: callback only uses the Vec pointer we pass.
        unsafe {
            EnumWindows(Some(menu_cb), &mut out as *mut Vec<WindowInfo> as LPARAM);
        }
        out
    }

    fn foreground(&self) -> Option<WindowInfo> {
        // SAFETY: no arguments.
        let h = unsafe { GetForegroundWindow() };
        if h.is_null() { None } else { window_info(h) }
    }
}
