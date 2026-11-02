//! Windows clipboard (CF_DIB image, CF_UNICODETEXT text).

use crate::util::to_wide;
use screenit_engine::Clipboard;
use screenit_raster::Raster;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

pub struct Win32Clipboard;

struct Open;

impl Open {
    fn new() -> Result<Self, String> {
        // The clipboard may be briefly held by another process; retry a few times.
        for _ in 0..10 {
            // SAFETY: null owner window is allowed.
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err("clipboard is busy".into())
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: balanced with a successful OpenClipboard.
        unsafe {
            CloseClipboard();
        }
    }
}

fn set_global(format: u32, bytes: &[u8]) -> Result<(), String> {
    // SAFETY: allocate movable memory, copy into it, and hand ownership to the clipboard only on
    // success; on failure we free it ourselves.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if mem.is_null() {
            return Err("out of memory".into());
        }
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            GlobalFree(mem);
            return Err("GlobalLock failed".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem as HANDLE).is_null() {
            GlobalFree(mem);
            return Err("SetClipboardData failed".into());
        }
    }
    Ok(())
}

impl Clipboard for Win32Clipboard {
    fn set_image(&self, img: &Raster) -> Result<(), String> {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: h as i32, // bottom-up
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: (w * h * 4) as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bytes = Vec::with_capacity(40 + w * h * 4);
        // SAFETY: BITMAPINFOHEADER is plain-old-data with no padding.
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                &header as *const BITMAPINFOHEADER as *const u8,
                std::mem::size_of::<BITMAPINFOHEADER>(),
            )
        });
        for y in (0..h).rev() {
            for x in 0..w {
                let p = img.get(x as i32, y as i32).unwrap_or([0, 0, 0, 255]);
                // Premultiply against white-less black so non-alpha-aware apps see the colours.
                bytes.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        let _guard = Open::new()?;
        // SAFETY: clipboard is open.
        unsafe {
            EmptyClipboard();
        }
        set_global(CF_DIB, &bytes)
    }

    fn get_image(&self) -> Result<Raster, String> {
        let _guard = Open::new()?;
        // SAFETY: clipboard open; the handle stays valid while locked and we copy out of it.
        unsafe {
            if IsClipboardFormatAvailable(CF_DIB) == 0 {
                return Err("clipboard has no image".into());
            }
            let h = GetClipboardData(CF_DIB);
            if h.is_null() {
                return Err("clipboard has no image".into());
            }
            let size = GlobalSize(h);
            let p = GlobalLock(h) as *const u8;
            if p.is_null() || size < std::mem::size_of::<BITMAPINFOHEADER>() {
                return Err("clipboard image unreadable".into());
            }
            let data = std::slice::from_raw_parts(p, size);
            let hdr = std::ptr::read_unaligned(data.as_ptr() as *const BITMAPINFOHEADER);
            let result = decode_dib(&hdr, data);
            GlobalUnlock(h);
            result
        }
    }

    fn set_text(&self, text: &str) -> Result<(), String> {
        let wide = to_wide(text);
        // SAFETY: u16 slice reinterpreted as bytes of twice the length.
        let bytes =
            unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        let _guard = Open::new()?;
        // SAFETY: clipboard is open.
        unsafe {
            EmptyClipboard();
        }
        set_global(CF_UNICODETEXT, bytes)
    }

    fn get_text(&self) -> Result<String, String> {
        let _guard = Open::new()?;
        // SAFETY: clipboard is open; the handle stays valid while locked and we copy out of it.
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return Err("clipboard has no text".into());
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                return Err("clipboard has no text".into());
            }
            let size = GlobalSize(h);
            let p = GlobalLock(h) as *const u16;
            if p.is_null() || size < 2 {
                return Err("clipboard text unreadable".into());
            }
            let n = (size / 2).min(1_000_000);
            let slice = std::slice::from_raw_parts(p, n);
            let end = slice.iter().position(|c| *c == 0).unwrap_or(slice.len());
            let text = String::from_utf16_lossy(&slice[..end]);
            GlobalUnlock(h);
            Ok(text)
        }
    }
}

fn decode_dib(hdr: &BITMAPINFOHEADER, data: &[u8]) -> Result<Raster, String> {
    let (w, h_raw, bpp) = (hdr.biWidth, hdr.biHeight, hdr.biBitCount);
    if w <= 0 || h_raw == 0 || !(bpp == 24 || bpp == 32) || hdr.biCompression != BI_RGB {
        return Err("unsupported clipboard bitmap format".into());
    }
    let (w, h) = (w as usize, h_raw.unsigned_abs() as usize);
    let bottom_up = h_raw > 0;
    let stride = (w * bpp as usize / 8).div_ceil(4) * 4;
    let off = hdr.biSize as usize + hdr.biClrUsed as usize * 4;
    if off.checked_add(stride * h).is_none_or(|e| e > data.len()) {
        return Err("clipboard bitmap is truncated".into());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src_row = if bottom_up { h - 1 - row } else { row };
        let line = &data[off + src_row * stride..];
        for x in 0..w {
            if bpp == 32 {
                rgba.extend_from_slice(&[line[x * 4 + 2], line[x * 4 + 1], line[x * 4], 255]);
            } else {
                rgba.extend_from_slice(&[line[x * 3 + 2], line[x * 3 + 1], line[x * 3], 255]);
            }
        }
    }
    Raster::from_rgba(w as u32, h as u32, rgba).map_err(|e| e.to_string())
}
