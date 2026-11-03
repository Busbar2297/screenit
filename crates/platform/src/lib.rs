//! Windows platform layer: the only crate allowed to use `unsafe`. Implements the capture
//! crate's `ScreenSource` and the engine's `Clipboard` with GDI/DWM/user32.
#![allow(unsafe_code)]
#![cfg(windows)]

pub mod audio;
pub mod autostart;
pub mod camera;
mod clipboard;
pub mod dup;
pub mod hotkeys;
mod icon_glyph;
pub mod input;
pub mod instance;
pub mod keyhook;
pub mod mf;
mod ocr;
mod playback;
mod screen;
pub mod shell;
mod uia;
mod util;
pub mod window;

pub use clipboard::Win32Clipboard;
pub use icon_glyph::rgba as app_icon_rgba;
pub use mf::{MfRecorder, MfVideoReader, MfVideoWriter};
pub use ocr::WinOcr;
pub use screen::Win32Screen;

use screenit_engine::Services;
use std::sync::Arc;

/// Opt in to per-monitor DPI awareness so coordinates are physical pixels. Call once at startup.
pub fn init_dpi_awareness() {
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    // SAFETY: plain FFI call with a documented constant; failure (already set) is harmless.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

struct Devices;

impl screenit_record::AvDevices for Devices {
    fn microphones(&self) -> Vec<String> {
        audio::microphones()
    }
    fn cameras(&self) -> Vec<String> {
        camera::cameras()
    }
}

/// Real services for the engine on this machine.
pub fn services() -> Services {
    init_dpi_awareness();
    let screen: Arc<dyn screenit_capture::ScreenSource> = Arc::new(Win32Screen::new());
    let rec_screen = screen.clone();
    Services {
        screen: Some(screen),
        clipboard: Some(Arc::new(Win32Clipboard)),
        ocr: Some(Arc::new(WinOcr)),
        llm: Some(Arc::new(screenit_llm::LmStudio)),
        camera: Some(Arc::new(camera::MfCamera::new(None))),
        devices: Some(Arc::new(Devices)),
        recorder: Some(Box::new(move || {
            Box::new(MfRecorder::new(rec_screen.clone()))
        })),
        video: Some(Arc::new(MfVideoReader::new())),
        video_writer: Some(Arc::new(MfVideoWriter)),
        player: Some(Arc::new(playback::MfTrackPlayer::new())),
        ..Services::default()
    }
}
