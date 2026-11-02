//! System fallback fonts for scripts egui's bundled fonts lack (Arabic, Japanese, Chinese).
//! Nothing is redistributed: the fonts come from the user's Windows installation.

use eframe::egui;
use std::sync::Arc;

fn candidates(code: &str) -> &'static [&'static str] {
    match code.split('-').next().unwrap_or(code) {
        "ar" => &["arial.ttf", "segoeui.ttf", "tahoma.ttf"],
        "ja" => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
        "zh" => &["msyh.ttc", "msyh.ttf", "simsun.ttc"],
        _ => &[],
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
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families.entry(family).or_default().push(key.clone());
        }
        break;
    }
    ctx.set_fonts(defs);
}
