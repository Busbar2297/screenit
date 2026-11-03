//! Password fields found through Windows UI Automation (`IsPassword`), so Hide secrets can blur
//! them without reading any pixels. Runs on its own COM thread with a deadline: a hung app must
//! not hang a capture.

use screenit_geom::Rect;
use std::sync::mpsc::channel;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_IsPasswordPropertyId,
};

fn variant_true() -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: VARIANT_BOOL(-1),
                },
            }),
        },
    }
}

/// Most elements read from one window.
const MAX_PER_WINDOW: i32 = 64;
const DEADLINE: Duration = Duration::from_millis(300);

/// Bounding boxes (virtual-screen pixels) of password edit controls in the given top-level
/// windows. Empty when nothing is found or the query takes longer than the deadline.
pub fn password_fields(hwnds: Vec<u64>) -> Vec<Rect> {
    if hwnds.is_empty() {
        return Vec::new();
    }
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(query(&hwnds).unwrap_or_default());
    });
    rx.recv_timeout(DEADLINE).unwrap_or_default()
}

fn query(hwnds: &[u64]) -> windows::core::Result<Vec<Rect>> {
    let mut out = Vec::new();
    // SAFETY: COM objects are created, used and dropped on this thread only. Every returned
    // pointer-like value is a COM smart pointer managed by the windows crate.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let auto: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
        let cond = auto.CreatePropertyCondition(UIA_IsPasswordPropertyId, &variant_true())?;
        for &id in hwnds {
            let hwnd = HWND(id as usize as *mut core::ffi::c_void);
            let Ok(root) = auto.ElementFromHandle(hwnd) else {
                continue;
            };
            let Ok(found) = root.FindAll(TreeScope_Descendants, &cond) else {
                continue;
            };
            let n = found.Length().unwrap_or(0).min(MAX_PER_WINDOW);
            for i in 0..n {
                let Ok(el) = found.GetElement(i) else {
                    continue;
                };
                let Ok(r) = el.CurrentBoundingRectangle() else {
                    continue;
                };
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                if w > 0 && h > 0 && r.left > -30000 {
                    out.push(Rect::new(r.left, r.top, w as u32, h as u32));
                }
            }
        }
    }
    Ok(out)
}
