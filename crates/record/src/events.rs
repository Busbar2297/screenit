//! Input event log: what the mouse and keyboard did during a recording, kept next to the video
//! (`clip.events.json`) so auto zoom, cursor styles and typing speed-up can be applied afterwards.
//!
//! Privacy: typed characters are never stored. Plain typing becomes a `Type` event (time only);
//! only chords with Ctrl/Alt/Win and non-character keys (Enter, Esc, F-keys...) keep a name.

use crate::RecordError;
use screenit_geom::{Point, Rect};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Most events kept for one recording; later ones are dropped.
pub const MAX_EVENTS: usize = 200_000;
/// Cursor moves closer than this (px) to the last logged move are skipped.
const MOVE_STEP: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Move,
    Down,
    Up,
    Wheel,
    /// A shortcut or non-character key; `chord` holds its name.
    Key,
    /// A character key. No text is kept.
    Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEvent {
    pub t_ms: u64,
    pub kind: EventKind,
    /// Screen position of the pointer.
    pub x: i32,
    pub y: i32,
    /// 0 left, 1 right, 2 middle (Down/Up only).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub button: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chord: String,
    /// Control under the pointer on a Down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLog {
    /// Recorded area on screen, when the log started.
    pub region: Rect,
    pub duration_ms: u64,
    pub events: Vec<InputEvent>,
}

impl EventLog {
    /// `a/clip.mp4` -> `a/clip.events.json`.
    pub fn path_for(video: &Path) -> PathBuf {
        video.with_extension("events.json")
    }

    pub fn save(&self, video: &Path) -> Result<PathBuf, RecordError> {
        let p = Self::path_for(video);
        let text = serde_json::to_string(self).map_err(|e| RecordError::Platform(e.to_string()))?;
        std::fs::write(&p, text).map_err(|e| RecordError::Platform(e.to_string()))?;
        Ok(p)
    }

    pub fn load(video: &Path) -> Result<Self, RecordError> {
        let p = Self::path_for(video);
        let text = std::fs::read_to_string(&p).map_err(|e| RecordError::Platform(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| RecordError::BadConfig(format!("events: {e}")))
    }

    /// Clicks (left, right or middle button going down), in time order.
    pub fn clicks(&self) -> impl Iterator<Item = &InputEvent> {
        self.events.iter().filter(|e| e.kind == EventKind::Down)
    }
}

/// One poll of the input devices.
#[derive(Clone, Debug, Default)]
pub struct InputSample {
    pub pos: Option<Point>,
    /// Bit 0 left, bit 1 right, bit 2 middle.
    pub buttons: u8,
    /// Virtual-key codes of keyboard keys that are down right now.
    pub keys: Vec<u8>,
}

const VK_SHIFT: u8 = 0x10;
const VK_CONTROL: u8 = 0x11;
const VK_MENU: u8 = 0x12;
const VK_LWIN: u8 = 0x5B;
const VK_RWIN: u8 = 0x5C;

fn is_modifier(vk: u8) -> bool {
    matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN)
        // left/right variants
        || (0xA0..=0xA5).contains(&vk)
}

/// Keys that put a character on screen (or erase one).
fn is_character(vk: u8) -> bool {
    matches!(vk,
        0x08 | 0x20 | 0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDF)
}

/// Readable name of a key.
pub fn key_name(vk: u8) -> String {
    match vk {
        0x08 => "backspace".into(),
        0x09 => "tab".into(),
        0x0D => "enter".into(),
        0x1B => "esc".into(),
        0x20 => "space".into(),
        0x21 => "pageup".into(),
        0x22 => "pagedown".into(),
        0x23 => "end".into(),
        0x24 => "home".into(),
        0x25 => "left".into(),
        0x26 => "up".into(),
        0x27 => "right".into(),
        0x28 => "down".into(),
        0x2D => "insert".into(),
        0x2E => "delete".into(),
        0x30..=0x39 | 0x41..=0x5A => char::from(vk).to_ascii_lowercase().to_string(),
        0x70..=0x87 => format!("f{}", vk - 0x6F),
        _ => format!("vk{vk:02x}"),
    }
}

/// Turns polls into events: edges of buttons and keys, and pointer moves.
#[derive(Default)]
pub struct EventSampler {
    log: EventLog,
    buttons: u8,
    keys: Vec<u8>,
    last_move: Option<Point>,
    last_pos: Point,
}

impl EventSampler {
    pub fn new(region: Rect) -> Self {
        Self {
            log: EventLog {
                region,
                ..EventLog::default()
            },
            ..Self::default()
        }
    }

    fn push(&mut self, e: InputEvent) {
        if self.log.events.len() < MAX_EVENTS {
            self.log.events.push(e);
        }
    }

    /// Feed one poll taken `t_ms` after the recording started. `rect_at` names the control under
    /// a point (called only on a button press).
    pub fn feed(&mut self, t_ms: u64, s: &InputSample, rect_at: &dyn Fn(Point) -> Option<Rect>) {
        if let Some(p) = s.pos {
            self.last_pos = p;
            let far = self.last_move.is_none_or(|q| {
                (i64::from(p.x) - i64::from(q.x)).abs() + (i64::from(p.y) - i64::from(q.y)).abs()
                    >= MOVE_STEP
            });
            if far {
                self.last_move = Some(p);
                self.push(self.event(t_ms, EventKind::Move, 0, String::new(), None));
            }
        }
        let pos = self.last_pos;
        for b in 0..3u8 {
            let (was, now) = (self.buttons & (1 << b) != 0, s.buttons & (1 << b) != 0);
            if now && !was {
                let rect = rect_at(pos);
                self.push(self.event(t_ms, EventKind::Down, b, String::new(), rect));
            } else if was && !now {
                self.push(self.event(t_ms, EventKind::Up, b, String::new(), None));
            }
        }
        self.buttons = s.buttons;
        let mods = |keys: &[u8]| {
            let mut m = Vec::new();
            if keys.iter().any(|k| matches!(*k, VK_CONTROL | 0xA2 | 0xA3)) {
                m.push("ctrl");
            }
            if keys.iter().any(|k| matches!(*k, VK_MENU | 0xA4 | 0xA5)) {
                m.push("alt");
            }
            if keys.iter().any(|k| matches!(*k, VK_SHIFT | 0xA0 | 0xA1)) {
                m.push("shift");
            }
            if keys.iter().any(|k| matches!(*k, VK_LWIN | VK_RWIN)) {
                m.push("win");
            }
            m
        };
        let held = mods(&s.keys);
        let shortcut_mod = held.iter().any(|m| *m != "shift");
        for &vk in &s.keys {
            if is_modifier(vk) || self.keys.contains(&vk) {
                continue;
            }
            if is_character(vk) && !shortcut_mod {
                self.push(self.event(t_ms, EventKind::Type, 0, String::new(), None));
            } else {
                let mut parts: Vec<String> = held.iter().map(|m| (*m).to_string()).collect();
                parts.push(key_name(vk));
                self.push(self.event(t_ms, EventKind::Key, 0, parts.join("+"), None));
            }
        }
        self.keys = s.keys.clone();
    }

    fn event(
        &self,
        t_ms: u64,
        kind: EventKind,
        button: u8,
        chord: String,
        rect: Option<Rect>,
    ) -> InputEvent {
        InputEvent {
            t_ms,
            kind,
            x: self.last_pos.x,
            y: self.last_pos.y,
            button,
            chord,
            rect,
        }
    }

    pub fn finish(mut self, duration_ms: u64) -> EventLog {
        self.log.duration_ms = duration_ms;
        self.log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: i32, y: i32, buttons: u8, keys: &[u8]) -> InputSample {
        InputSample {
            pos: Some(Point::new(x, y)),
            buttons,
            keys: keys.to_vec(),
        }
    }

    fn rect(_: Point) -> Option<Rect> {
        Some(Rect::new(1, 2, 3, 4))
    }

    #[test]
    fn click_edges_carry_the_control_rect() {
        let mut m = EventSampler::new(Rect::new(0, 0, 100, 100));
        m.feed(0, &s(10, 10, 0, &[]), &rect);
        m.feed(40, &s(10, 10, 1, &[]), &rect);
        m.feed(80, &s(10, 10, 1, &[]), &rect);
        m.feed(120, &s(10, 10, 0, &[]), &rect);
        let log = m.finish(200);
        let kinds: Vec<_> = log.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec![EventKind::Move, EventKind::Down, EventKind::Up]);
        assert_eq!(log.events[1].rect, Some(Rect::new(1, 2, 3, 4)));
        assert_eq!(log.clicks().count(), 1);
    }

    #[test]
    fn tiny_moves_are_skipped() {
        let mut m = EventSampler::new(Rect::default());
        m.feed(0, &s(0, 0, 0, &[]), &rect);
        m.feed(10, &s(1, 1, 0, &[]), &rect);
        m.feed(20, &s(9, 0, 0, &[]), &rect);
        assert_eq!(m.finish(30).events.len(), 2);
    }

    #[test]
    fn typing_keeps_no_characters_but_shortcuts_keep_names() {
        let mut m = EventSampler::new(Rect::default());
        m.feed(0, &s(0, 0, 0, &[0x41]), &rect); // a
        m.feed(10, &s(0, 0, 0, &[]), &rect);
        m.feed(20, &s(0, 0, 0, &[VK_CONTROL, 0x53]), &rect); // ctrl+s
        m.feed(30, &s(0, 0, 0, &[VK_CONTROL, 0x53]), &rect); // held: no repeat
        m.feed(40, &s(0, 0, 0, &[0x0D]), &rect); // enter
        let log = m.finish(50);
        let keys: Vec<_> = log
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Key | EventKind::Type))
            .map(|e| (e.kind, e.chord.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (EventKind::Type, ""),
                (EventKind::Key, "ctrl+s"),
                (EventKind::Key, "enter")
            ]
        );
        let json = serde_json::to_string(&log).unwrap_or_default();
        assert!(!json.contains("\"a\""));
    }

    #[test]
    fn log_round_trips_and_bad_files_error() {
        let dir = std::env::temp_dir().join(format!("scr_ev_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("clip.mp4");
        let mut m = EventSampler::new(Rect::new(5, 6, 7, 8));
        m.feed(0, &s(3, 4, 1, &[]), &rect);
        let log = m.finish(99);
        let p = log.save(&video).unwrap();
        assert!(p.ends_with("clip.events.json"));
        assert_eq!(EventLog::load(&video).unwrap(), log);
        std::fs::write(&p, "{nope").unwrap();
        assert!(EventLog::load(&video).is_err());
        assert!(EventLog::load(&dir.join("none.mp4")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_is_capped() {
        let mut m = EventSampler::new(Rect::default());
        for i in 0..(MAX_EVENTS as u64 + 10) {
            let x = (i % 2) as i32 * 50;
            m.feed(i, &s(x, 0, 0, &[]), &rect);
        }
        assert_eq!(m.finish(0).events.len(), MAX_EVENTS);
    }
}
