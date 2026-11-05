//! Test fixture: a borderless topmost window painted with the deterministic testkit pattern.
//! Usage: `screenit-fixture [x y w h]` (defaults 100 100 320 200). Prints
//! `READY <hwnd> <x> <y> <w> <h>` once painted, exits when stdin closes or after 120 s.
#![allow(unsafe_code, clippy::chunks_exact_to_as_chunks)]

#[cfg(windows)]
mod form;
#[cfg(windows)]
mod win {
    use screenit_testkit::pattern;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, DIB_RGB_COLORS, EndPaint, InvalidateRect,
        PAINTSTRUCT, SetDIBitsToDevice, UpdateWindow,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, MSG, PM_REMOVE,
        PeekMessageW, PostQuitMessage, RegisterClassW, SW_SHOWNA, ShowWindow, TranslateMessage,
        WM_DESTROY, WM_MOUSEWHEEL, WM_PAINT, WM_QUIT, WNDCLASSW, WS_EX_TOPMOST, WS_POPUP,
    };

    static mut BGRA: Vec<u8> = Vec::new();
    static mut DIM: (u32, u32) = (0, 0);
    // Scroll mode: content is pages times taller than the window and the wheel moves it.
    static mut OFFSET: i32 = 0;
    static mut MAX_OFFSET: i32 = 0;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: statics are only touched on the UI thread (single-threaded fixture).
        unsafe {
            match m {
                WM_PAINT => {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    let hdc = BeginPaint(h, &mut ps);
                    let (cw, ch) = DIM;
                    let mut bi: BITMAPINFO = std::mem::zeroed();
                    bi.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: cw as i32,
                        biHeight: -(ch as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    };
                    let bgra = &*std::ptr::addr_of!(BGRA);
                    let skip = OFFSET as usize * cw as usize * 4;
                    let bgra = &bgra[skip.min(bgra.len())..];
                    SetDIBitsToDevice(
                        hdc,
                        0,
                        0,
                        cw,
                        ch,
                        0,
                        0,
                        0,
                        ch,
                        bgra.as_ptr() as *const c_void,
                        &bi,
                        DIB_RGB_COLORS,
                    );
                    EndPaint(h, &ps);
                    0
                }
                WM_MOUSEWHEEL => {
                    let delta = ((w >> 16) & 0xffff) as u16 as i16 as i32;
                    let step = -(delta / 120) * 40;
                    OFFSET = (OFFSET + step).clamp(0, MAX_OFFSET);
                    InvalidateRect(h, std::ptr::null(), 0);
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

    pub fn run() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|s| s == "--form") {
            let a: Vec<i32> = args.iter().filter_map(|s| s.parse().ok()).collect();
            match a[..] {
                [x, y, w, h] if w > 0 && h > 0 => super::form::run(x, y, w, h),
                _ => super::form::run(100, 100, 520, 380),
            }
            return;
        }
        let pages: u32 = args
            .iter()
            .find_map(|s| s.strip_prefix("--scroll=")?.parse().ok())
            .unwrap_or(1);
        let a: Vec<i32> = args.iter().filter_map(|s| s.parse().ok()).collect();
        let (x, y, w, h) = match a[..] {
            [x, y, w, h] if w > 0 && h > 0 => (x, y, w, h),
            _ => (100, 100, 320, 200),
        };
        let img = pattern(w as u32, h as u32 * pages.max(1));
        let mut bgra = Vec::with_capacity(img.data().len());
        for p in img.data().chunks_exact(4) {
            bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        // SAFETY: single-threaded FFI sequence; the window class/name buffers outlive the calls.
        unsafe {
            BGRA = bgra;
            DIM = (w as u32, h as u32);
            MAX_OFFSET = h * (pages.max(1) as i32 - 1);
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("screenItFixture");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: inst,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let title = wide("screenIt Fixture");
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
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
            ShowWindow(hwnd, SW_SHOWNA);
            UpdateWindow(hwnd);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            println!("READY {} {x} {y} {w} {h}", hwnd as usize);

            // Exit when the controlling test closes our stdin (or after 120 s).
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
                if start.elapsed().as_secs() > 120 {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    win::run();
}
