//! Translations for screenIt.
//!
//! One flat TOML file per locale (`locales/<code>.toml`, `key = "text"`). English is embedded and
//! is the fallback for any missing key. Files in the user's `locales` folder (and next to the
//! executable) override the bundled text, so translations can be edited and reloaded at runtime.
//!
//! Text for right-to-left locales is shaped (Arabic letter forms) and put in visual order by
//! [`display`], because the UI toolkit lays text out left to right.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// `(code, native name, bundled file)`. Order is the order shown in the language picker.
pub const LOCALES: &[(&str, &str, &str)] = &[
    ("en", "English", include_str!("../locales/en.toml")),
    ("ar", "العربية", include_str!("../locales/ar.toml")),
    ("bg", "Български", include_str!("../locales/bg.toml")),
    ("cs", "Čeština", include_str!("../locales/cs.toml")),
    ("da", "Dansk", include_str!("../locales/da.toml")),
    ("de", "Deutsch", include_str!("../locales/de.toml")),
    ("el", "Ελληνικά", include_str!("../locales/el.toml")),
    ("es", "Español", include_str!("../locales/es.toml")),
    ("et", "Eesti", include_str!("../locales/et.toml")),
    ("fi", "Suomi", include_str!("../locales/fi.toml")),
    ("fr", "Français", include_str!("../locales/fr.toml")),
    ("ga", "Gaeilge", include_str!("../locales/ga.toml")),
    ("hr", "Hrvatski", include_str!("../locales/hr.toml")),
    ("hu", "Magyar", include_str!("../locales/hu.toml")),
    ("it", "Italiano", include_str!("../locales/it.toml")),
    ("ja", "日本語", include_str!("../locales/ja.toml")),
    ("lt", "Lietuvių", include_str!("../locales/lt.toml")),
    ("lv", "Latviešu", include_str!("../locales/lv.toml")),
    ("mt", "Malti", include_str!("../locales/mt.toml")),
    ("nl", "Nederlands", include_str!("../locales/nl.toml")),
    ("pl", "Polski", include_str!("../locales/pl.toml")),
    ("pt", "Português", include_str!("../locales/pt.toml")),
    ("ro", "Română", include_str!("../locales/ro.toml")),
    ("ru", "Русский", include_str!("../locales/ru.toml")),
    ("sk", "Slovenčina", include_str!("../locales/sk.toml")),
    ("sl", "Slovenščina", include_str!("../locales/sl.toml")),
    ("sv", "Svenska", include_str!("../locales/sv.toml")),
    (
        "zh-Hans",
        "简体中文",
        include_str!("../locales/zh-Hans.toml"),
    ),
];

/// Locale codes written right to left.
const RTL: &[&str] = &["ar", "he", "fa", "ur"];

/// Parse a locale file: flat `key = "text"` pairs (nested tables are flattened with dots).
pub fn parse(text: &str) -> Result<HashMap<String, String>, String> {
    fn walk(
        prefix: &str,
        v: &toml::Value,
        out: &mut HashMap<String, String>,
    ) -> Result<(), String> {
        match v {
            toml::Value::String(s) => {
                out.insert(prefix.to_string(), s.clone());
                Ok(())
            }
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, v, out)?;
                }
                Ok(())
            }
            _ => Err(format!("`{prefix}` must be text")),
        }
    }
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut out = HashMap::new();
    walk("", &toml::Value::Table(table), &mut out)?;
    Ok(out)
}

/// Replace `{name}` placeholders.
pub fn fill(text: &str, args: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Names of the `{placeholders}` in a string, sorted and de-duplicated.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j)
                if after[..j]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                names.push(after[..j].to_string());
                rest = &after[j + 1..];
            }
            _ => rest = after,
        }
    }
    names.sort();
    names.dedup();
    names
}

fn english() -> &'static HashMap<String, String> {
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    EN.get_or_init(|| parse(LOCALES[0].2).unwrap_or_default())
}

/// The bundled strings of `code` (English when unknown).
pub fn bundled(code: &str) -> HashMap<String, String> {
    LOCALES
        .iter()
        .find(|(c, _, _)| c.eq_ignore_ascii_case(code))
        .and_then(|(_, _, text)| parse(text).ok())
        .unwrap_or_default()
}

pub fn is_rtl_code(code: &str) -> bool {
    let base = code.split('-').next().unwrap_or(code);
    RTL.iter().any(|r| r.eq_ignore_ascii_case(base))
}

fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Shape and reorder `text` for a left-to-right text renderer. Text without right-to-left
/// characters is returned unchanged.
pub fn display(text: &str) -> String {
    if !text.chars().any(is_rtl_char) {
        return text.to_string();
    }
    // Ligature glyphs (lam-alef) are missing from several system fonts and render as two
    // swapped letters, so shape without ligatures: lam and alef then join as ordinary forms.
    static SHAPER: std::sync::OnceLock<ar_reshaper::ArabicReshaper> = std::sync::OnceLock::new();
    let shaper = SHAPER.get_or_init(|| {
        ar_reshaper::ArabicReshaper::new(ar_reshaper::ReshaperConfig {
            support_ligatures: false,
            ..Default::default()
        })
    });
    let shaped = shaper.reshape(text);
    let info = unicode_bidi::BidiInfo::new(&shaped, None);
    let mut out = String::with_capacity(shaped.len());
    for para in &info.paragraphs {
        out.push_str(&info.reorder_line(para, para.range.clone()));
    }
    out
}

/// A loaded locale: bundled text plus any user overrides.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    code: String,
    strings: HashMap<String, String>,
}

impl Catalog {
    /// Load `code` with overrides from `dirs` (later directories win). A malformed override file
    /// is skipped and reported; the bundled text stays in effect.
    pub fn load(code: &str, dirs: &[PathBuf]) -> (Self, Vec<String>) {
        let mut strings = bundled(code);
        let mut problems = Vec::new();
        for dir in dirs {
            let path = dir.join(format!("{code}.toml"));
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match parse(&text) {
                Ok(over) => strings.extend(over),
                Err(e) => problems.push(format!("{}: {e}", path.display())),
            }
        }
        (
            Self {
                code: code.to_string(),
                strings,
            },
            problems,
        )
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Text for `key` in this locale, then English, then the key itself. Not shaped.
    pub fn get(&self, key: &str) -> String {
        self.strings
            .get(key)
            .or_else(|| english().get(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    pub fn has(&self, key: &str) -> bool {
        self.strings.contains_key(key)
    }
}

struct State {
    catalog: Catalog,
    dirs: Vec<PathBuf>,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| {
        RwLock::new(State {
            catalog: Catalog::load("en", &[]).0,
            dirs: Vec::new(),
        })
    })
}

/// Folders searched for override files, lowest priority first.
pub fn set_dirs(dirs: Vec<PathBuf>) {
    if let Ok(mut s) = state().write() {
        s.dirs = dirs;
    }
}

/// Switch language. Unknown codes are rejected; malformed override files are reported but the
/// bundled text of the locale stays active.
pub fn set_locale(code: &str) -> Result<(), String> {
    let dirs = state().read().map(|s| s.dirs.clone()).unwrap_or_default();
    let known = LOCALES.iter().any(|(c, _, _)| *c == code)
        || dirs
            .iter()
            .any(|d| d.join(format!("{code}.toml")).is_file());
    if !known {
        return Err(format!("unknown language `{code}`"));
    }
    let (catalog, problems) = Catalog::load(code, &dirs);
    if let Ok(mut s) = state().write() {
        s.catalog = catalog;
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// Re-read the active locale from disk.
pub fn reload() -> Result<(), String> {
    let code = locale();
    set_locale(&code)
}

pub fn locale() -> String {
    state()
        .read()
        .map(|s| s.catalog.code().to_string())
        .unwrap_or_else(|_| "en".into())
}

pub fn is_rtl() -> bool {
    is_rtl_code(&locale())
}

/// Translated, display-ready text for `key`.
pub fn t(key: &str) -> String {
    let raw = state()
        .read()
        .map(|s| s.catalog.get(key))
        .unwrap_or_else(|_| key.to_string());
    display(&raw)
}

/// Like [`t`] with `{name}` placeholders filled in before shaping.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let raw = state()
        .read()
        .map(|s| s.catalog.get(key))
        .unwrap_or_else(|_| key.to_string());
    display(&fill(&raw, args))
}

/// `(code, native name)` of every bundled locale plus any extra `<code>.toml` in the override
/// folders (named by their `meta.name` key, or the code).
pub fn locales() -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = LOCALES
        .iter()
        .map(|(c, n, _)| ((*c).to_string(), (*n).to_string()))
        .collect();
    let dirs = state().read().map(|s| s.dirs.clone()).unwrap_or_default();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            let Some(code) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if list.iter().any(|(c, _)| c == code) {
                continue;
            }
            let name = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| parse(&t).ok())
                .and_then(|m| m.get("meta.name").cloned())
                .unwrap_or_else(|| code.to_string());
            list.push((code.to_string(), name));
        }
    }
    list
}

/// Write the bundled files of every locale into `dir` so they can be edited (existing files are
/// kept). Returns how many were written.
pub fn export_bundled(dir: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let mut n = 0;
    for (code, _, text) in LOCALES {
        let p = dir.join(format!("{code}.toml"));
        if !p.exists() {
            std::fs::write(p, text)?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("scit-i18n-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_bundled_locale_parses_and_uses_known_keys() {
        let en = parse(LOCALES[0].2).unwrap();
        assert!(en.len() > 60);
        for (code, _, text) in LOCALES {
            let m = parse(text).unwrap_or_else(|e| panic!("{code}: {e}"));
            for (k, v) in &m {
                let base = en
                    .get(k)
                    .unwrap_or_else(|| panic!("{code}: key `{k}` is not in en.toml"));
                assert_eq!(
                    placeholders(v),
                    placeholders(base),
                    "{code}: placeholders of `{k}` differ from English"
                );
            }
        }
    }

    #[test]
    fn every_bundled_locale_is_complete() {
        let en = parse(LOCALES[0].2).unwrap();
        for (code, _, text) in LOCALES {
            let m = parse(text).unwrap();
            let missing: Vec<_> = en.keys().filter(|k| !m.contains_key(*k)).collect();
            assert!(missing.is_empty(), "{code} lacks {missing:?}");
        }
    }

    #[test]
    fn missing_keys_fall_back_to_english_then_the_key() {
        let (c, _) = Catalog::load("de", &[]);
        let en = Catalog::load("en", &[]).0;
        assert_eq!(c.get("no.such.key"), "no.such.key");
        assert!(!c.get("bar.record").is_empty());
        assert_eq!(
            Catalog::load("zz", &[]).0.get("bar.record"),
            en.get("bar.record")
        );
    }

    #[test]
    fn user_files_override_bundled_text_in_priority_order() {
        let (a, b) = (tmp("a"), tmp("b"));
        std::fs::write(
            a.join("de.toml"),
            "bar.record = \"A\"\nbar.library = \"A-lib\"\n",
        )
        .unwrap();
        std::fs::write(b.join("de.toml"), "bar.record = \"B\"\n").unwrap();
        let (c, problems) = Catalog::load("de", &[a, b]);
        assert!(problems.is_empty());
        assert_eq!(c.get("bar.record"), "B");
        assert_eq!(c.get("bar.library"), "A-lib");
    }

    #[test]
    fn malformed_override_is_reported_and_bundled_text_kept() {
        let d = tmp("bad");
        std::fs::write(d.join("de.toml"), "bar.record = [oops").unwrap();
        let (c, problems) = Catalog::load("de", std::slice::from_ref(&d));
        assert_eq!(problems.len(), 1);
        assert_eq!(
            c.get("bar.record"),
            Catalog::load("de", &[]).0.get("bar.record")
        );
        std::fs::write(d.join("de.toml"), "bar.record = 5").unwrap();
        assert_eq!(Catalog::load("de", &[d]).1.len(), 1);
    }

    #[test]
    fn placeholders_are_filled() {
        assert_eq!(fill("{a} x {b}", &[("a", "1"), ("b", "2")]), "1 x 2");
        assert_eq!(placeholders("{w} × {h} {w}"), vec!["h", "w"]);
        assert!(placeholders("no braces").is_empty());
    }

    #[test]
    fn rtl_text_is_shaped_and_reordered_but_latin_is_untouched() {
        assert_eq!(display("Capture 1920"), "Capture 1920");
        let logical = "\u{645}\u{631}\u{62D}\u{628}\u{627}"; // مرحبا
        let shown = display(logical);
        assert_ne!(shown, logical);
        assert!(shown.chars().all(is_rtl_char), "presentation forms");
        // Visual order of an isolated word is the reverse of its logical order.
        assert_eq!(shown.chars().count(), 5);
        // Numbers inside RTL text stay readable left to right.
        let mixed = display("\u{645}\u{631}\u{62D}\u{628}\u{627} 1920");
        assert!(mixed.starts_with("1920"));
    }

    #[test]
    fn rtl_codes() {
        assert!(is_rtl_code("ar"));
        assert!(is_rtl_code("ar-SA"));
        assert!(!is_rtl_code("de"));
    }

    #[test]
    fn global_switching_and_unknown_codes() {
        assert!(set_locale("zz-nope").is_err());
        assert!(set_locale("fr").is_ok());
        assert_eq!(locale(), "fr");
        assert!(!is_rtl());
        assert!(set_locale("ar").is_ok());
        assert!(is_rtl());
        assert!(set_locale("en").is_ok());
        assert_eq!(locales().len(), LOCALES.len());
    }
}
