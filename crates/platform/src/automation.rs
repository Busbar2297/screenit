//! Windows side of the AI tutorial engine: UI Automation snapshots, synthetic input through
//! `SendInput`, and starting / finding / focusing applications.
//!
//! Safety nets that live here: every UIA query has a deadline and element caps, a hung app cannot
//! hang a run, the kill hotkey (Ctrl+Shift+Pause) and any mouse movement away from where the
//! driver last put the cursor stop the run, and the Windows key is never sent.

use crate::util::{from_wide, id_from_hwnd};
use screenit_geom::Rect;
use screenit_tutor::keys::{Key, parse_chord};
use screenit_tutor::{
    AppLauncher, InputDriver, MouseButton, Shot, TutorError, TutorResult, UiElement, UiSnapshot,
    UiTree, WindowInfo,
};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_HasKeyboardFocusPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsPasswordPropertyId, UIA_NamePropertyId, UIA_ValuePatternId,
};
use windows::core::Interface;
use windows_sys::Win32::Foundation::{HWND as RawHwnd, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT,
    VK_MENU, VK_NEXT, VK_PAUSE, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
    VkKeyScanW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_RESTORE, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};

fn raw(id: u64) -> RawHwnd {
    id as usize as RawHwnd
}

// ------------------------------------------------------------------ windows

fn window_rect(h: RawHwnd) -> Rect {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `r` is a valid RECT of the size passed; `h` is only used as a handle.
    let ok = unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        ) == 0
    };
    if !ok {
        // SAFETY: valid out pointer.
        unsafe { GetWindowRect(h, &mut r) };
    }
    Rect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    )
}

fn window_info(h: RawHwnd) -> WindowInfo {
    let mut buf = [0u16; 256];
    let mut pid = 0u32;
    // SAFETY: buffers are valid for the sizes given.
    let n = unsafe {
        GetWindowThreadProcessId(h, &mut pid);
        GetWindowTextW(h, buf.as_mut_ptr(), 256)
    };
    WindowInfo {
        hwnd: id_from_hwnd(h),
        pid,
        title: from_wide(&buf[..n.max(0) as usize]),
        rect: window_rect(h),
    }
}

/// Visible, titled, top-level, non-tool windows of other processes, front to back.
fn top_windows() -> Vec<WindowInfo> {
    unsafe extern "system" fn cb(h: RawHwnd, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the `Vec` we pass to EnumWindows; the call is synchronous.
        let out = unsafe { &mut *(lparam as *mut Vec<WindowInfo>) };
        // SAFETY: plain queries on a handle EnumWindows gave us.
        let (vis, ex, own) = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            (
                IsWindowVisible(h) != 0,
                GetWindowLongPtrW(h, GWL_EXSTYLE),
                pid == GetCurrentProcessId(),
            )
        };
        if vis && !own && ex & WS_EX_TOOLWINDOW as isize == 0 {
            let w = window_info(h);
            if !w.title.is_empty() && !w.rect.is_empty() {
                out.push(w);
            }
        }
        1
    }
    let mut out: Vec<WindowInfo> = Vec::new();
    // SAFETY: `out` outlives the synchronous call.
    unsafe { EnumWindows(Some(cb), &mut out as *mut Vec<WindowInfo> as LPARAM) };
    out
}

// ------------------------------------------------------------------ UI Automation

pub struct Win32Ui;

const MAX_ELEMENTS: usize = 400;
const MAX_DEPTH: u8 = 14;
const SNAPSHOT_DEADLINE: Duration = Duration::from_millis(3000);

fn role_name(t: i32) -> &'static str {
    match t {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        _ => "Custom",
    }
}

fn read_element(el: &IUIAutomationElement, depth: u8) -> Option<UiElement> {
    // SAFETY: COM calls on a live element owned by this thread; failures map to defaults.
    unsafe {
        let r = el.CachedBoundingRectangle().ok()?;
        let ct = el.CachedControlType().map(|c| c.0).unwrap_or(0);
        let password = el.CachedIsPassword().map(|b| b.as_bool()).unwrap_or(false);
        let mut value = String::new();
        if !password
            && matches!(ct, 50003 | 50004 | 50030 | 50016 | 50015)
            && let Ok(p) = el.GetCurrentPattern(UIA_ValuePatternId)
            && let Ok(vp) = p.cast::<IUIAutomationValuePattern>()
        {
            value = vp
                .CurrentValue()
                .map(|b| b.to_string())
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        Some(UiElement {
            id: 0,
            role: role_name(ct).to_string(),
            name: el
                .CurrentName()
                .map(|b| b.to_string())
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect(),
            automation_id: el
                .CurrentAutomationId()
                .map(|b| b.to_string())
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect(),
            rect: if w > 0 && h > 0 && r.left > -30000 {
                Rect::new(r.left, r.top, w as u32, h as u32)
            } else {
                Rect::default()
            },
            enabled: el.CachedIsEnabled().map(|b| b.as_bool()).unwrap_or(true),
            focused: el
                .CurrentHasKeyboardFocus()
                .map(|b| b.as_bool())
                .unwrap_or(false),
            value,
            password,
            depth,
            path: String::new(),
        })
    }
}

fn walk(
    walker: &IUIAutomationTreeWalker,
    cr: &IUIAutomationCacheRequest,
    el: &IUIAutomationElement,
    depth: u8,
    out: &mut Vec<UiElement>,
    deadline: Instant,
) {
    if out.len() >= MAX_ELEMENTS || depth > MAX_DEPTH || Instant::now() > deadline {
        return;
    }
    if let Some(e) = read_element(el, depth) {
        out.push(e);
    }
    // SAFETY: COM calls on live elements; an error ends the sibling loop.
    let mut child = unsafe { walker.GetFirstChildElementBuildCache(el, cr) }.ok();
    while let Some(c) = child {
        walk(walker, cr, &c, depth + 1, out, deadline);
        if out.len() >= MAX_ELEMENTS || Instant::now() > deadline {
            break;
        }
        // SAFETY: as above.
        child = unsafe { walker.GetNextSiblingElementBuildCache(&c, cr) }.ok();
    }
}

fn snapshot_blocking(hwnd: u64) -> Result<Vec<UiElement>, String> {
    // SAFETY: COM objects are created, used and dropped on this thread only.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        // One cache request fetches every property in a single cross-process call per element.
        let cr = auto.CreateCacheRequest().map_err(|e| e.to_string())?;
        for id in [
            UIA_NamePropertyId,
            UIA_AutomationIdPropertyId,
            UIA_ControlTypePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_IsEnabledPropertyId,
            UIA_HasKeyboardFocusPropertyId,
            UIA_IsPasswordPropertyId,
        ] {
            cr.AddProperty(id).map_err(|e| e.to_string())?;
        }
        let root = auto
            .ElementFromHandleBuildCache(HWND(hwnd as usize as *mut core::ffi::c_void), &cr)
            .map_err(|e| e.to_string())?;
        let walker = auto.ControlViewWalker().map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        walk(
            &walker,
            &cr,
            &root,
            0,
            &mut out,
            Instant::now() + SNAPSHOT_DEADLINE,
        );
        Ok(out)
    }
}

impl UiTree for Win32Ui {
    fn snapshot(&self, window: u64) -> TutorResult<UiSnapshot> {
        // SAFETY: handle validity query only.
        if unsafe { IsWindow(raw(window)) } == 0 {
            return Err(TutorError::Ui("the window is gone".into()));
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(snapshot_blocking(window));
        });
        let elements = rx
            .recv_timeout(SNAPSHOT_DEADLINE + Duration::from_millis(1500))
            .map_err(|_| TutorError::Ui("the app did not answer UI Automation in time".into()))?
            .map_err(TutorError::Ui)?;
        Ok(UiSnapshot {
            window: window_info(raw(window)),
            elements,
        }
        .finish())
    }

    fn other_windows(&self) -> Vec<WindowInfo> {
        top_windows()
    }

    fn foreground(&self) -> Option<WindowInfo> {
        // SAFETY: plain query.
        let h = unsafe { GetForegroundWindow() };
        (!h.is_null()).then(|| window_info(h))
    }

    fn is_alive(&self, window: u64) -> bool {
        // SAFETY: handle validity query only.
        unsafe { IsWindow(raw(window)) != 0 }
    }

    fn capture(&self, window: u64) -> Option<Shot> {
        use screenit_capture::ScreenSource;
        let img = crate::screen::Win32Screen::new().grab_window(window).ok()?;
        if img.is_empty() {
            return None;
        }
        let width = img.width();
        let height = img.height();
        let rgba = img.into_data();
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))?;
        (rgba.len() == bytes).then_some(Shot {
            width,
            height,
            rgba,
        })
    }
}

// ------------------------------------------------------------------ input

/// Cursor deviation (pixels) from where the driver put it that counts as the user taking over.
/// Generous on purpose: a resting hand, a bumped desk or a high-DPI rounding error must not end a run.
const TAKEOVER_PX: i64 = 60;

struct Watch {
    /// Packed cursor position this driver last claimed (`x << 32 | y`), or `NONE`.
    last: AtomicI64,
    tripped: AtomicBool,
    /// Ignore takeover samples until this millisecond, so our own jump can land.
    ignore_until: AtomicU64,
    started: Instant,
    abort: Mutex<Option<Arc<AtomicBool>>>,
}

pub struct Win32Input {
    watch: Arc<Watch>,
}

fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    // SAFETY: valid out pointer.
    let ok = unsafe { GetCursorPos(&mut p) } != 0;
    ok.then_some((p.x, p.y))
}

fn drifted(home: (i32, i32), cursor: (i32, i32)) -> bool {
    (i64::from(cursor.0) - i64::from(home.0))
        .abs()
        .max((i64::from(cursor.1) - i64::from(home.1)).abs())
        > TAKEOVER_PX
}

/// Samples the cursor while a run is blocked (a model call, a click pause) and latches a stop
/// once the pointer stays away from where this driver put it.
fn cursor_watch(w: Arc<Watch>) {
    let mut away = 0u8;
    loop {
        std::thread::sleep(Duration::from_millis(20));
        if w.tripped.load(Ordering::SeqCst) {
            away = 0;
            continue;
        }
        let Some(cur) = cursor_pos() else {
            continue;
        };
        let home = w.last.load(Ordering::SeqCst);
        if home == NONE {
            w.last.store(pack(cur.0, cur.1), Ordering::SeqCst);
            away = 0;
            continue;
        }
        let now = w.started.elapsed().as_millis() as u64;
        if now < w.ignore_until.load(Ordering::SeqCst) {
            // Our own jump is landing. Trust where the cursor really ended up: with several
            // monitors at different scales Windows can place it a few pixels off the request.
            w.last.store(pack(cur.0, cur.1), Ordering::SeqCst);
            away = 0;
            continue;
        }
        if drifted(unpack(home), cur) {
            away = away.saturating_add(1);
            if away >= 2 {
                w.tripped.store(true, Ordering::SeqCst);
            }
        } else {
            away = 0;
        }
    }
}

const NONE: i64 = i64::MIN;

impl Default for Win32Input {
    fn default() -> Self {
        Self::new()
    }
}

fn pack(x: i32, y: i32) -> i64 {
    (i64::from(x) << 32) | i64::from(y as u32)
}

fn unpack(v: i64) -> (i32, i32) {
    ((v >> 32) as i32, v as u32 as i32)
}

fn key_down(vk: u16) -> bool {
    // SAFETY: stateless query.
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

fn send(inputs: &[INPUT]) -> TutorResult<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of INPUT structs of the size passed.
    let n = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err(TutorError::Input(
            "Windows blocked the input (is the target app running elevated?)".into(),
        ))
    }
}

fn mouse(flags: u32, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn kbd(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

impl Win32Input {
    pub fn new() -> Self {
        let watch = Arc::new(Watch {
            last: AtomicI64::new(NONE),
            tripped: AtomicBool::new(false),
            ignore_until: AtomicU64::new(0),
            started: Instant::now(),
            abort: Mutex::new(None),
        });
        let watcher = Arc::clone(&watch);
        std::thread::spawn(move || cursor_watch(watcher));
        Self { watch }
    }

    fn abort_flag(&self) -> bool {
        self.watch
            .abort
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|f| f.load(Ordering::SeqCst))
    }

    fn pause_check(&self, ms: u64) -> TutorResult<()> {
        let mut left = ms;
        while left > 0 {
            self.stop_check()?;
            let slice = left.min(20);
            pause(slice);
            left -= slice;
        }
        self.stop_check()
    }

    fn put_cursor(&self, x: i32, y: i32) -> TutorResult<()> {
        // SAFETY: metrics queries.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            )
        };
        let nx = ((i64::from(x) - i64::from(vx)) * 65535 / i64::from(vw - 1)).clamp(0, 65535);
        let ny = ((i64::from(y) - i64::from(vy)) * 65535 / i64::from(vh - 1)).clamp(0, 65535);
        let now = self.watch.started.elapsed().as_millis() as u64;
        self.watch
            .ignore_until
            .store(now.saturating_add(60), Ordering::SeqCst);
        self.watch.last.store(pack(x, y), Ordering::SeqCst);
        send(&[mouse(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            nx as i32,
            ny as i32,
            0,
        )])
    }

    fn press(&self, button: MouseButton, down: bool) -> TutorResult<()> {
        let f = match (button, down) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        send(&[mouse(f, 0, 0, 0)])
    }

    fn stop_check(&self) -> TutorResult<()> {
        if self.abort_flag() || self.interrupted() {
            Err(TutorError::Aborted(
                "the user took over the mouse or pressed Stop".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn vk_of(key: &Key) -> Option<(u16, bool)> {
    Some(match key {
        Key::Named(n) => (
            match *n {
                "enter" => VK_RETURN,
                "esc" => VK_ESCAPE,
                "tab" => VK_TAB,
                "space" => VK_SPACE,
                "backspace" => VK_BACK,
                "delete" => VK_DELETE,
                "insert" => VK_INSERT,
                "home" => VK_HOME,
                "end" => VK_END,
                "pageup" => VK_PRIOR,
                "pagedown" => VK_NEXT,
                "up" => VK_UP,
                "down" => VK_DOWN,
                "left" => VK_LEFT,
                "right" => VK_RIGHT,
                _ => return None,
            },
            false,
        ),
        Key::Function(n) => (VK_F1 + u16::from(n.saturating_sub(1)), false),
        Key::Char(c) => {
            let mut buf = [0u16; 2];
            let u = c.encode_utf16(&mut buf);
            if u.len() != 1 {
                return None;
            }
            // SAFETY: plain layout query.
            let r = unsafe { VkKeyScanW(u[0]) };
            if r == -1 {
                return None;
            }
            ((r & 0xff) as u16, r & 0x100 != 0)
        }
    })
}

impl InputDriver for Win32Input {
    fn move_to(&self, x: i32, y: i32) -> TutorResult<()> {
        self.stop_check()?;
        self.put_cursor(x, y)
    }

    fn click(&self, x: i32, y: i32, button: MouseButton, count: u8) -> TutorResult<()> {
        self.stop_check()?;
        self.put_cursor(x, y)?;
        self.pause_check(40)?;
        for _ in 0..count.clamp(1, 3) {
            self.press(button, true)?;
            self.pause_check(35)?;
            self.press(button, false)?;
            self.pause_check(60)?;
        }
        Ok(())
    }

    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> TutorResult<()> {
        self.stop_check()?;
        self.put_cursor(from.0, from.1)?;
        self.pause_check(80)?;
        self.press(MouseButton::Left, true)?;
        let steps = 24;
        for i in 1..=steps {
            let x = from.0 + (to.0 - from.0) * i / steps;
            let y = from.1 + (to.1 - from.1) * i / steps;
            if let Err(e) = self.stop_check().and_then(|()| self.put_cursor(x, y)) {
                let _ = self.press(MouseButton::Left, false);
                return Err(e);
            }
            self.pause_check(14)?;
        }
        self.pause_check(60)?;
        self.press(MouseButton::Left, false)
    }

    fn type_text(&self, text: &str, delay_ms: u32) -> TutorResult<()> {
        for ch in text.chars().take(2000) {
            self.stop_check()?;
            if ch == '\n' {
                send(&[kbd(VK_RETURN, 0, 0), kbd(VK_RETURN, 0, KEYEVENTF_KEYUP)])?;
            } else if ch == '\t' {
                send(&[kbd(VK_TAB, 0, 0), kbd(VK_TAB, 0, KEYEVENTF_KEYUP)])?;
            } else if !ch.is_control() {
                let mut buf = [0u16; 2];
                for u in ch.encode_utf16(&mut buf) {
                    send(&[
                        kbd(0, *u, KEYEVENTF_UNICODE),
                        kbd(0, *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ])?;
                }
            }
            if delay_ms > 0 {
                let jitter = u64::from(ch as u32 % 7) * u64::from(delay_ms) / 14;
                self.pause_check(u64::from(delay_ms.min(500)) + jitter)?;
            }
        }
        Ok(())
    }

    fn key(&self, chord: &str) -> TutorResult<()> {
        self.stop_check()?;
        let c = parse_chord(chord)?;
        let (vk, needs_shift) = vk_of(&c.key)
            .ok_or_else(|| TutorError::Input(format!("no key for chord `{chord}`")))?;
        let shift = c.shift || needs_shift;
        let mut down: Vec<u16> = Vec::new();
        if c.ctrl {
            down.push(VK_CONTROL);
        }
        if c.alt {
            down.push(VK_MENU);
        }
        if shift {
            down.push(VK_SHIFT);
        }
        let mut seq: Vec<INPUT> = down.iter().map(|&m| kbd(m, 0, 0)).collect();
        seq.push(kbd(vk, 0, 0));
        seq.push(kbd(vk, 0, KEYEVENTF_KEYUP));
        seq.extend(down.iter().rev().map(|&m| kbd(m, 0, KEYEVENTF_KEYUP)));
        send(&seq)?;
        self.pause_check(60)?;
        Ok(())
    }

    fn scroll(&self, x: i32, y: i32, dy: i32) -> TutorResult<()> {
        self.stop_check()?;
        self.put_cursor(x, y)?;
        self.pause_check(30)?;
        // Positive `dy` scrolls the content down, which is a negative wheel delta.
        for _ in 0..dy.unsigned_abs().min(100) {
            let d = if dy > 0 { -120 } else { 120 };
            send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, d)])?;
            self.pause_check(40)?;
        }
        Ok(())
    }

    fn interrupted(&self) -> bool {
        if self.watch.tripped.load(Ordering::SeqCst) {
            return true;
        }
        let hotkey = key_down(VK_PAUSE) && key_down(VK_CONTROL) && key_down(VK_SHIFT);
        let last = self.watch.last.load(Ordering::SeqCst);
        let now = self.watch.started.elapsed().as_millis() as u64;
        let settling = now < self.watch.ignore_until.load(Ordering::SeqCst);
        let moved =
            !settling && last != NONE && cursor_pos().is_some_and(|p| drifted(unpack(last), p));
        if hotkey || moved {
            self.watch.tripped.store(true, Ordering::SeqCst);
        }
        self.watch.tripped.load(Ordering::SeqCst)
    }

    fn begin(&self) {
        self.watch.tripped.store(false, Ordering::SeqCst);
        self.watch.ignore_until.store(0, Ordering::SeqCst);
        if let Some((x, y)) = cursor_pos() {
            self.watch.last.store(pack(x, y), Ordering::SeqCst);
        } else {
            self.watch.last.store(NONE, Ordering::SeqCst);
        }
    }

    fn bind_abort(&self, flag: &Arc<AtomicBool>) {
        *self
            .watch
            .abort
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(flag));
    }

    fn user_stop(&self) {
        self.watch.tripped.store(true, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------ launcher

pub struct Win32Launcher;

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// A program is started directly. A document path or a URL is opened by the shell.
/// A bare name is a program (`notepad` finds `notepad.exe` on PATH), never Explorer.
fn open_with_shell(target: &str) -> bool {
    let t = target.trim();
    let l = lower(t);
    if l.starts_with("http://") || l.starts_with("https://") || l.starts_with("ms-") {
        return true;
    }
    if l.ends_with(".exe") || l.ends_with(".bat") || l.ends_with(".cmd") || l.ends_with(".com") {
        return false;
    }
    t.contains('\\') || t.contains('/')
}

impl AppLauncher for Win32Launcher {
    fn launch(&self, target: &str, args: &str) -> TutorResult<WindowInfo> {
        let bad = |m: &str| Err(TutorError::Launch(m.to_string()));
        if target.trim().is_empty() || target.len() > 500 {
            return bad("target must be 1..=500 characters");
        }
        if target.chars().any(|c| c.is_control() || c == '"') || args.chars().any(char::is_control)
        {
            return bad(
                "target and args must not contain control characters or quotes in the target",
            );
        }
        let before: Vec<u64> = top_windows().iter().map(|w| w.hwnd).collect();
        let program = !open_with_shell(target);
        let spawned = if program {
            std::process::Command::new(target.trim())
                .args(args.split_whitespace().take(32))
                .spawn()
        } else {
            std::process::Command::new("explorer.exe")
                .arg(target.trim())
                .spawn()
        };
        let child = spawned.map_err(|e| {
            TutorError::Launch(format!(
                "`{target}` did not start ({e}). Give the full path to the .exe, or set Window title to an app that is already open and leave App to launch empty."
            ))
        })?;
        let pid = child.id();
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            pause(300);
            let wins = top_windows();
            let hit = wins
                .iter()
                .find(|w| program && w.pid == pid)
                .or_else(|| wins.iter().find(|w| !before.contains(&w.hwnd)));
            if let Some(w) = hit {
                pause(700);
                return Ok(window_info(raw(w.hwnd)));
            }
        }
        Err(TutorError::Launch(format!(
            "`{target}` showed no window within 15 s"
        )))
    }

    fn find_window(&self, title: &str) -> Option<WindowInfo> {
        let t = lower(title.trim());
        if t.is_empty() {
            return None;
        }
        top_windows()
            .into_iter()
            .find(|w| lower(&w.title).contains(&t))
    }

    fn focus(&self, window: u64) -> TutorResult<()> {
        let h = raw(window);
        // SAFETY: handle queries and the documented foreground calls.
        unsafe {
            if IsWindow(h) == 0 {
                return Err(TutorError::Launch("the window is gone".into()));
            }
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
        }
        // A tap on Alt lets SetForegroundWindow through when we are not the foreground process.
        let _ = send(&[kbd(VK_MENU, 0, 0), kbd(VK_MENU, 0, KEYEVENTF_KEYUP)]);
        for _ in 0..5 {
            // SAFETY: documented call on a valid handle.
            let ok = unsafe { SetForegroundWindow(h) != 0 && GetForegroundWindow() == h };
            if ok {
                return Ok(());
            }
            pause(120);
        }
        Err(TutorError::Launch(
            "could not bring the window to the front".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::open_with_shell;

    #[test]
    fn cursor_drift_over_60px_is_a_takeover() {
        assert!(!super::drifted((10, 10), (40, 38)));
        assert!(super::drifted((10, 10), (75, 10)));
        assert!(super::drifted((10, 10), (10, -55)));
    }

    #[test]
    fn a_program_name_is_not_opened_with_explorer() {
        assert!(!open_with_shell("notepad"));
        assert!(!open_with_shell("C:\\Windows\\notepad.exe"));
        assert!(open_with_shell("https://example.com"));
        assert!(open_with_shell("C:\\Users\\me\\Notes.pdf"));
    }
}
