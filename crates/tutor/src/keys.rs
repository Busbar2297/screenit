//! Key chord parsing (`ctrl+shift+s`, `enter`, `alt+f4`), independent of the OS.

use crate::error::{TutorError, TutorResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character (lower-cased letters).
    Char(char),
    /// `enter`, `esc`, `tab`, `space`, `backspace`, `delete`, `insert`, `home`, `end`,
    /// `pageup`, `pagedown`, `up`, `down`, `left`, `right`.
    Named(&'static str),
    /// `f1`..`f12`.
    Function(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

const NAMED: &[(&str, &str)] = &[
    ("enter", "enter"),
    ("return", "enter"),
    ("esc", "esc"),
    ("escape", "esc"),
    ("tab", "tab"),
    ("space", "space"),
    ("backspace", "backspace"),
    ("delete", "delete"),
    ("del", "delete"),
    ("insert", "insert"),
    ("home", "home"),
    ("end", "end"),
    ("pageup", "pageup"),
    ("pagedown", "pagedown"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
];

pub fn parse_chord(s: &str) -> TutorResult<Chord> {
    let bad = |m: &str| Err(TutorError::Input(format!("key chord `{s}`: {m}")));
    if s.trim().is_empty() || s.len() > 60 {
        return bad("empty or too long");
    }
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    let mut key: Option<Key> = None;
    let norm = if s.trim() == "+" {
        "plus".to_string()
    } else {
        s.replace("++", "+plus")
    };
    for part in norm.split('+') {
        let p = part.trim().to_lowercase();
        match p.as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" => alt = true,
            "win" | "windows" | "meta" | "cmd" | "super" => {
                return bad("the Windows key is not allowed");
            }
            "" => return bad("empty key name"),
            _ => {
                if key.is_some() {
                    return bad("more than one key");
                }
                key = Some(parse_key(&p).ok_or_else(|| {
                    TutorError::Input(format!("key chord `{s}`: unknown key `{p}`"))
                })?);
            }
        }
    }
    match key {
        Some(key) => Ok(Chord {
            ctrl,
            shift,
            alt,
            key,
        }),
        None => bad("no key"),
    }
}

fn parse_key(p: &str) -> Option<Key> {
    if p == "plus" {
        return Some(Key::Char('+'));
    }
    if let Some((_, n)) = NAMED.iter().find(|(a, _)| *a == p) {
        return Some(Key::Named(n));
    }
    if let Some(num) = p.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&num).then_some(Key::Function(num));
    }
    let mut it = p.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if !c.is_control() => Some(Key::Char(c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chords() {
        let c = parse_chord("Ctrl+Shift+S").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, Key::Char('s'));
        assert_eq!(parse_chord("alt+F4").unwrap().key, Key::Function(4));
        assert_eq!(parse_chord("Enter").unwrap().key, Key::Named("enter"));
        assert_eq!(parse_chord("ctrl++").unwrap().key, Key::Char('+'));
        assert_eq!(parse_chord("esc").unwrap().key, Key::Named("esc"));
    }

    #[test]
    fn rejects_bad_chords() {
        for s in [
            "",
            "  ",
            "ctrl",
            "ctrl+a+b",
            "win+r",
            "f13",
            "ctrl+banana",
            "a".repeat(61).as_str(),
        ] {
            assert!(parse_chord(s).is_err(), "{s}");
        }
    }
}
