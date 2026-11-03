//! Scrolling capture from the UI: pick an area on the overlay, then take one
//! `capture.scroll.step` per frame until the content stops moving, a page limit or Esc.

use crate::{UiApp, View};
use eframe::egui;
use screenit_capture::{FrozenScreen, Selection};
use screenit_geom::Rect;
use serde_json::json;
use std::time::{Duration, Instant};

/// Pages taken before stopping on its own.
const MAX_PAGES: usize = 30;
/// Wait for the overlay to disappear from the screen before the first grab.
const START_DELAY: Duration = Duration::from_millis(450);

pub(crate) struct ScrollRun {
    rect: Rect,
    started: bool,
    next_at: Instant,
    pages: usize,
    /// Esc has to be released once before it counts (it may still be down from the overlay).
    esc_armed: bool,
}

impl UiApp {
    /// The overlay picked the area (or window) to scroll.
    pub(crate) fn begin_scroll(&mut self, frozen: &FrozenScreen, sel: Option<Selection>) {
        let rect = match sel {
            Some(Selection::Region { rect }) => Some(rect),
            Some(Selection::Window { id }) => frozen
                .windows_snapshot()
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.rect),
            _ => None,
        };
        match rect.filter(|r| r.w >= 16 && r.h >= 16) {
            Some(rect) => {
                self.scroll = Some(ScrollRun {
                    rect,
                    started: false,
                    next_at: Instant::now() + START_DELAY,
                    pages: 0,
                    esc_armed: false,
                });
                // Show the notice now so it is already hidden from capture at the first grab.
                self.toast = Some(crate::toast::Notice::scrolling(1));
            }
            None => self.settle_after_capture(),
        }
    }

    /// Advance a running scrolling capture by one step. Called once per frame.
    pub(crate) fn drive_scroll(&mut self, ctx: &egui::Context) {
        let Some(run) = self.scroll.as_mut() else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(30));
        // Escape counts only once the start delay is over, so an old key press that is still
        // latched (or still held from the overlay) does not end the capture before it begins.
        let esc = screenit_platform::input::escape_pressed();
        if !run.started {
            run.esc_armed = Instant::now() >= run.next_at && !esc;
        }
        let stop = esc && run.started && run.esc_armed;
        if Instant::now() < run.next_at && !stop {
            return;
        }
        let rect = run.rect;
        if !run.started {
            run.started = true;
            let r = self.run(
                "capture.scroll.begin",
                json!({"x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h}),
            );
            if r.is_none() {
                self.abort_scroll();
                return;
            }
            self.set_scroll_progress(1);
            return;
        }
        let mut done = stop;
        if !done {
            match self.run(
                "capture.scroll.step",
                json!({"notches": 3, "settle_ms": 120}),
            ) {
                Some(v) => {
                    let frames = v["frames"].as_u64().unwrap_or(0) as usize;
                    done = v["end"] == true || frames >= MAX_PAGES;
                    self.set_scroll_progress(frames);
                }
                None => {
                    self.abort_scroll();
                    return;
                }
            }
        }
        if done {
            self.finish_scroll(ctx);
        }
    }

    fn set_scroll_progress(&mut self, pages: usize) {
        if let Some(run) = self.scroll.as_mut() {
            run.pages = pages;
            run.next_at = Instant::now() + Duration::from_millis(30);
        }
        self.toast = Some(crate::toast::Notice::scrolling(pages));
    }

    fn abort_scroll(&mut self) {
        self.scroll = None;
        self.toast = None;
        let _ = self.session.dispatch("capture.scroll.cancel", &json!({}));
        self.settle_after_capture();
    }

    fn finish_scroll(&mut self, ctx: &egui::Context) {
        self.scroll = None;
        self.toast = None;
        let clip = self.session.prefs.copy_to_clipboard;
        match self.run("capture.scroll.finish", json!({"clipboard": clip})) {
            Some(v) => {
                self.show_capture(&v);
                self.fit = true;
                if self.session.settings.opens_editor(None) {
                    self.enter_view(ctx, View::Editor);
                    self.restore_window(ctx);
                } else {
                    self.settle_after_capture();
                }
            }
            None => self.settle_after_capture(),
        }
    }
}
