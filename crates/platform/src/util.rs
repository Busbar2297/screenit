use windows_sys::Win32::Foundation::HWND;

pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn hwnd_from_id(id: u64) -> HWND {
    id as usize as HWND
}

pub fn id_from_hwnd(h: HWND) -> u64 {
    h as usize as u64
}
