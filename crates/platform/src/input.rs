//! Global input state used by recording effects and capture cancel.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};

fn down(vk: u16) -> bool {
    // SAFETY: stateless query; the high bit means "currently down".
    unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
}

pub fn left_button_down() -> bool {
    down(VK_LBUTTON)
}

pub fn right_button_down() -> bool {
    down(VK_RBUTTON)
}

/// Mouse buttons down now: bit 0 left, bit 1 right, bit 2 middle.
pub fn button_mask() -> u8 {
    u8::from(down(VK_LBUTTON))
        | (u8::from(down(VK_RBUTTON)) << 1)
        | (u8::from(down(VK_MBUTTON)) << 2)
}

/// Virtual-key codes of keyboard keys down now (mouse buttons excluded).
pub fn keys_down() -> Vec<u8> {
    (0x08u8..=0xFE).filter(|&vk| down(u16::from(vk))).collect()
}

/// Escape pressed anywhere, regardless of which window has keyboard focus.
pub fn escape_down() -> bool {
    down(VK_ESCAPE)
}

/// True when Escape is down now or was pressed since the previous call. A short tap that lands
/// while the caller was busy (a scrolling capture step takes half a second) is not lost.
pub fn escape_pressed() -> bool {
    // SAFETY: stateless query; bit 0 means "pressed since the last call", bit 15 "down now".
    unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE)) as u16 & 0x8001 != 0 }
}
