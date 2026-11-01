#![windows_subsystem = "windows"]
//! screenIt desktop app: egui UI + global hotkeys + tray icon + control channel.

use screenit_engine::Session;
use screenit_ui_egui::{UiEvent, Waker};
use std::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

fn data_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("screenIt")
}

/// A small camera-ish glyph so the tray entry is recognisable without shipping an asset.
fn tray_icon_image() -> Option<Icon> {
    let n = 32u32;
    let mut rgba = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let r = (dx * dx + dy * dy).sqrt();
            let px = if r < 7.0 {
                [255, 255, 255, 255]
            } else if r < 14.0 {
                [0, 120, 215, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, n, n).ok()
}

fn main() {
    use screenit_platform::instance::{Instance, acquire};
    let activation = match acquire() {
        Instance::First(a) => a,
        Instance::Existing => return, // the running instance brings its window forward
    };
    let dir = data_dir();
    let services = screenit_platform::services();
    let mut session = Session::new(services);
    if let Ok(lib) = screenit_library::Library::open(&dir.join("library")) {
        session.library = lib;
    }
    session.prefs.auto_save_dir = Some(screenit_platform::shell::output_dir());
    let presets = session.presets.clone();

    let result = screenit_ui_egui::run(session, move |waker: Waker| {
        let (tx, rx) = mpsc::channel::<UiEvent>();
        let (t, w) = (tx.clone(), waker.clone());
        activation.on_activate(move || {
            let _ = t.send(UiEvent::Show);
            w.wake();
        });

        // Global hotkeys from the presets. Plain Print Screen goes through a low-level keyboard
        // hook instead, so it works even while another capture tool has registered the key.
        let mut keys = Vec::new();
        let mut names = Vec::new();
        let mut print_preset = None;
        for p in &presets {
            let Some(h) = p.hotkey.as_deref() else {
                continue;
            };
            if h.eq_ignore_ascii_case("PrintScreen") {
                print_preset.get_or_insert_with(|| p.name.clone());
            } else if let Ok(hk) = screenit_capture::parse_hotkey(h) {
                keys.push(hk);
                names.push(p.name.clone());
            }
        }
        if let Some(name) = print_preset {
            screenit_platform::window::release_print_screen_from_snipping();
            let (t, w) = (tx.clone(), waker.clone());
            let t = std::sync::Mutex::new(t);
            let hooked = screenit_platform::keyhook::spawn_print_screen_hook(move || {
                if let Ok(t) = t.lock() {
                    let _ = t.send(UiEvent::RunPreset(name.clone()));
                }
                w.wake();
            });
            if !hooked {
                let _ = tx.send(UiEvent::Notice(
                    "Print Screen could not be hooked; use the Capture button or tray menu".into(),
                ));
            }
        }
        let (hk_tx, hk_rx) = mpsc::channel::<usize>();
        let w = waker.clone();
        let failed = screenit_platform::hotkeys::spawn(keys, hk_tx, move || w.wake());
        if !failed.is_empty() {
            let taken: Vec<&String> = failed.iter().filter_map(|i| names.get(*i)).collect();
            let _ = tx.send(UiEvent::Notice(format!(
                "Hotkeys in use by another application: {taken:?}"
            )));
        }
        let (t, w) = (tx.clone(), waker.clone());
        std::thread::spawn(move || {
            for id in hk_rx {
                if let Some(name) = names.get(id) {
                    let _ = t.send(UiEvent::RunPreset(name.clone()));
                    w.wake();
                }
            }
        });
        // Control channel.
        let (t, w) = (tx.clone(), waker.clone());
        let server = screenit_automation::start(&dir, move |line| {
            let (rtx, rrx) = mpsc::channel();
            if t.send(UiEvent::Control(line, rtx)).is_err() {
                return r#"{"ok":false,"error":{"code":"closed","message":"app is shutting down"}}"#.into();
            }
            w.wake();
            rrx.recv_timeout(std::time::Duration::from_secs(60))
                .unwrap_or_else(|_| {
                    r#"{"ok":false,"error":{"code":"timeout","message":"UI did not answer"}}"#
                        .into()
                })
        });
        if let Err(e) = &server {
            eprintln!("control channel disabled: {e}");
        }

        // Tray icon: keep it alive for the life of the process.
        let menu = Menu::new();
        let open = MenuItem::new("Open screenIt", true, None);
        let capture = MenuItem::new("Capture (All-in-One)", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let _ = menu.append_items(&[&open, &capture, &quit]);
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("screenIt");
        if let Some(icon) = tray_icon_image() {
            builder = builder.with_icon(icon);
        }
        if let Ok(tray) = builder.build() {
            std::mem::forget(tray);
        }
        let (open_id, capture_id, quit_id) =
            (open.id().clone(), capture.id().clone(), quit.id().clone());
        let (t, w) = (tx, waker);
        std::thread::spawn(move || {
            for ev in MenuEvent::receiver() {
                let msg = if ev.id == open_id {
                    UiEvent::Show
                } else if ev.id == capture_id {
                    UiEvent::RunPreset("All-in-One".into())
                } else if ev.id == quit_id {
                    UiEvent::Quit
                } else {
                    continue;
                };
                let _ = t.send(msg);
                w.wake();
            }
        });
        rx
    });
    if let Err(e) = result {
        eprintln!("screenIt failed to start: {e}");
        std::process::exit(1);
    }
}
