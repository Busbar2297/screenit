//! `--form` mode: a small real Win32 form (label, edit, buttons, checkbox, list box, password
//! box, status line) so tutorials can be rehearsed and recorded against genuine controls through
//! UI Automation and `SendInput`. Control ids double as UIA automation ids:
//! 101 item edit, 102 Add, 103 Clear, 104 list, 105 Uppercase checkbox, 106 status, 107 password.
//! Usage: `screenit-fixture --form [x y w h]`. Prints `READY <hwnd> ...` like the pattern mode.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, GetStockObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, ES_AUTOHSCROLL, ES_PASSWORD, GetDlgItem, GetWindowTextW, HMENU, LB_ADDSTRING,
    LB_RESETCONTENT, LBS_NOTIFY, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW,
    PostQuitMessage, QS_ALLINPUT, RegisterClassW, SW_SHOW, SendMessageW, SetForegroundWindow,
    SetWindowTextW, ShowWindow, TranslateMessage, WM_COMMAND, WM_DESTROY, WM_QUIT, WM_SETFONT,
    WNDCLASSW, WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const ID_EDIT: isize = 101;
const ID_ADD: isize = 102;
const ID_CLEAR: isize = 103;
const ID_LIST: isize = 104;
const ID_UPPER: isize = 105;
const ID_STATUS: isize = 106;
const ID_PASSWORD: isize = 107;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text_of(h: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buffer is valid for 512 u16.
    let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), 512) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn child(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    id: isize,
    r: (i32, i32, i32, i32),
) {
    let (c, t) = (wide(class), wide(text));
    // SAFETY: wide strings outlive the call; parent is a valid window.
    unsafe {
        let h = CreateWindowExW(
            0,
            c.as_ptr(),
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            r.0,
            r.1,
            r.2,
            r.3,
            parent,
            id as HMENU,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null::<c_void>(),
        );
        SendMessageW(h, WM_SETFONT, GetStockObject(DEFAULT_GUI_FONT) as usize, 1);
    }
}

unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: handles come from the window being served; buffers are local.
    unsafe {
        match m {
            WM_COMMAND => {
                let id = (w & 0xffff) as isize;
                let status = GetDlgItem(h, ID_STATUS as i32);
                if id == ID_ADD {
                    let edit = GetDlgItem(h, ID_EDIT as i32);
                    let mut t = text_of(edit);
                    if t.trim().is_empty() {
                        SetWindowTextW(status, wide("Nothing to add").as_ptr());
                        return 0;
                    }
                    let upper = GetDlgItem(h, ID_UPPER as i32);
                    if SendMessageW(upper, BM_GETCHECK, 0, 0) == 1 {
                        t = t.to_uppercase();
                    }
                    SendMessageW(
                        GetDlgItem(h, ID_LIST as i32),
                        LB_ADDSTRING,
                        0,
                        wide(&t).as_ptr() as isize,
                    );
                    SetWindowTextW(edit, wide("").as_ptr());
                    SetWindowTextW(status, wide(&format!("Added: {t}")).as_ptr());
                } else if id == ID_CLEAR {
                    SendMessageW(GetDlgItem(h, ID_LIST as i32), LB_RESETCONTENT, 0, 0);
                    SetWindowTextW(status, wide("Cleared").as_ptr());
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

pub fn run(x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: single-threaded FFI sequence; strings outlive the calls.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("screenItFormFixture");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: inst,
            lpszClassName: class.as_ptr(),
            hbrBackground: 16 as _, // COLOR_BTNFACE + 1
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("screenIt Form Fixture");
        let (px, py) = if x == i32::MIN {
            (CW_USEDEFAULT, CW_USEDEFAULT)
        } else {
            (x, y)
        };
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            px,
            py,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            eprintln!("CreateWindowExW failed");
            std::process::exit(1);
        }
        child(hwnd, "STATIC", "Item:", 0, 200, (16, 20, 60, 22));
        child(
            hwnd,
            "EDIT",
            "",
            WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            ID_EDIT,
            (80, 16, 220, 26),
        );
        child(
            hwnd,
            "BUTTON",
            "Add",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_ADD,
            (312, 16, 80, 26),
        );
        child(
            hwnd,
            "BUTTON",
            "Clear",
            WS_TABSTOP | BS_PUSHBUTTON as u32,
            ID_CLEAR,
            (400, 16, 80, 26),
        );
        child(
            hwnd,
            "BUTTON",
            "Uppercase",
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            ID_UPPER,
            (80, 54, 140, 24),
        );
        child(
            hwnd,
            "LISTBOX",
            "",
            WS_BORDER | WS_VSCROLL | LBS_NOTIFY as u32,
            ID_LIST,
            (16, 90, 464, 160),
        );
        child(hwnd, "STATIC", "Ready", 0, ID_STATUS, (16, 262, 464, 22));
        child(hwnd, "STATIC", "Password:", 0, 201, (16, 296, 70, 22));
        child(
            hwnd,
            "EDIT",
            "",
            WS_BORDER | WS_TABSTOP | ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
            ID_PASSWORD,
            (96, 292, 204, 26),
        );
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        println!("READY {} {x} {y} {w} {h}", hwnd as usize);

        std::thread::spawn(|| {
            let mut s = String::new();
            while std::io::stdin()
                .read_line(&mut s)
                .map(|n| n > 0)
                .unwrap_or(false)
            {
                s.clear();
            }
            std::process::exit(0);
        });
        let start = std::time::Instant::now();
        let mut msg: MSG = std::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if start.elapsed().as_secs() > 300 {
                return;
            }
            // Wake as soon as a message arrives so UI Automation calls are answered at once.
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 20, QS_ALLINPUT);
        }
    }
}
