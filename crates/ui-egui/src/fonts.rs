//! System fallback fonts for scripts egui's bundled fonts lack (Arabic, Japanese, Chinese).
//! Nothing is redistributed: the fonts come from the user's Windows installation.

use eframe::egui;
use std::sync::Arc;

fn candidates(code: &str) -> &'static [&'static str] {
    match code.split('-').next().unwrap_or(code) {
        // Tahoma and Arial contain the Arabic presentation forms the shaper emits. Segoe UI
        // relies on OpenType shaping, which this renderer does not do, so it is not used.
        "ar" => &["tahoma.ttf", "arial.ttf", "trad.ttf"],
        "ja" => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
        "zh" => &["msyh.ttc", "msyh.ttf", "simsun.ttc"],
        // Kept as a fallback so Arabic text (a toast, a grab) still draws in other languages.
        _ => &["tahoma.ttf", "arial.ttf"],
    }
}

/// Install the egui defaults plus the first available system font for `code`.
pub fn install(ctx: &egui::Context, code: &str) {
    let mut defs = egui::FontDefinitions::default();
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    for name in candidates(code) {
        let Ok(bytes) = std::fs::read(dir.join(name)) else {
            continue;
        };
        let key = format!("system-{name}");
        defs.font_data
            .insert(key.clone(), Arc::new(egui::FontData::from_owned(bytes)));
        let primary = code.split('-').next().unwrap_or(code) == "ar";
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = defs.families.entry(family).or_default();
            if primary {
                // First, so Arabic presentation forms are taken from this font. The bundled
                // font has no Arabic, and a later fallback is skipped when an earlier font
                // claims the codepoint with an empty box.
                list.insert(0, key.clone());
            } else {
                list.push(key.clone());
            }
        }
        break;
    }
    ctx.set_fonts(defs);
}
