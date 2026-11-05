//! The pointer: mouse and keyboard for whatever window is actually in front.
//!
//! The model does not name a control that may have vanished. It points at the picture it was just
//! shown, in coordinates from 0 to 1000 (top-left to bottom-right of that window). This maps back
//! to the screen whatever size the picture was shrunk to. A title change on the same window is
//! normal. A dialog is followed, and when it closes the pointer returns to the app it started on.

use crate::error::{TutorError, TutorResult};
use crate::player::Player;
use crate::script::{Selector, center};
use crate::traits::{Shot, UiElement, UiSnapshot, WindowInfo};
use screenit_geom::{Point, Rect};

/// What the model should look at for its next move.
pub(crate) struct Scene {
    pub snap: UiSnapshot,
    pub shot: Option<Shot>,
    /// Empty when this is the same window as last time.
    pub changed: String,
}

/// Move the player onto the window the app is showing now.
///
/// Same process: always, including a new title or a new document window. Another process: only
/// right after one of our actions, and only while our own window still exists, so a dialog we
/// just opened is followed and a random app the user switched to is not. A dead window sends us
/// back to the app this run started on.
pub(crate) fn track(player: &mut Player, just_acted: bool) -> Option<String> {
    let home_pid = player.guard.home_pid;
    let alive = player.rig.ui.is_alive(player.window.hwnd);
    if let Some(front) = player.rig.ui.foreground() {
        if front.hwnd != 0 && front.hwnd == player.window.hwnd {
            player.window.title = front.title.clone();
            player.window.rect = front.rect;
            return None;
        }
        let same_app = front.pid != 0 && front.pid == home_pid;
        let dialog = just_acted
            && alive
            && front.pid != 0
            && !front.title.is_empty()
            && player
                .rig
                .ui
                .other_windows()
                .iter()
                .any(|w| w.hwnd == front.hwnd);
        if same_app || dialog {
            return Some(adopt(player, front));
        }
    }
    if alive {
        return None;
    }
    let home = player
        .rig
        .ui
        .other_windows()
        .into_iter()
        .find(|w| w.pid == home_pid && player.rig.ui.is_alive(w.hwnd))
        .or_else(|| {
            let hwnd = player.guard.home_window;
            player
                .rig
                .ui
                .is_alive(hwnd)
                .then(|| player.rig.ui.snapshot(hwnd).ok())
                .flatten()
                .map(|s| s.window)
        });
    home.map(|w| adopt(player, w))
}

fn adopt(player: &mut Player, w: WindowInfo) -> String {
    let title = if w.title.is_empty() {
        "untitled".into()
    } else {
        w.title.clone()
    };
    player.guard.target_window = w.hwnd;
    player.guard.target_pid = w.pid;
    player.window = w;
    title
}

/// One look: follow the window, snapshot it, and say if the title or the window changed.
pub(crate) fn see(
    player: &mut Player,
    previous: Option<&WindowInfo>,
    just_acted: bool,
) -> TutorResult<Scene> {
    track(player, just_acted);
    let snap = match player.rig.ui.snapshot(player.window.hwnd) {
        Ok(s) => s,
        Err(_) => {
            track(player, true);
            player.rig.ui.snapshot(player.window.hwnd).map_err(|_| {
                TutorError::Ui("the window went away and the app has no window to return to".into())
            })?
        }
    };
    player.window = snap.window.clone();
    player.guard.target_window = snap.window.hwnd;
    player.guard.target_pid = snap.window.pid;
    let changed = match previous {
        Some(p) if p.hwnd != snap.window.hwnd => format!(
            "The window \"{}\" is gone. Now showing \"{}\". Decide again from this picture.",
            clip(&p.title, 80),
            clip(&snap.window.title, 80)
        ),
        Some(p) if p.title != snap.window.title => format!(
            "The window title changed from \"{}\" to \"{}\". Same window; keep going.",
            clip(&p.title, 80),
            clip(&snap.window.title, 80)
        ),
        _ => String::new(),
    };
    let shot = player.rig.ui.capture(snap.window.hwnd);
    Ok(Scene {
        snap,
        shot,
        changed,
    })
}

/// After a click, wait until the title and the window stop changing, then return that picture.
/// A title that changes once and settles is a success, not an error.
pub(crate) fn settle(player: &mut Player) -> TutorResult<UiSnapshot> {
    let mut last: Option<(u64, String)> = None;
    let mut snap = None;
    for i in 0..5 {
        let scene = see(player, None, true)?;
        let key = (scene.snap.window.hwnd, scene.snap.window.title.clone());
        if last.as_ref() == Some(&key) {
            return Ok(scene.snap);
        }
        last = Some(key);
        snap = Some(scene.snap);
        if i + 1 < 5 {
            player.rig.pace.sleep_ms(100);
        }
    }
    snap.ok_or_else(|| TutorError::Ui("the app closed".into()))
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `x` and `y` are 0..=1000 across the window picture. The result is a screen point inside it.
pub(crate) fn picture_point(window: Rect, x: i32, y: i32) -> (i32, i32) {
    let x = x.clamp(0, 1000);
    let y = y.clamp(0, 1000);
    let px = i64::from(window.x) + i64::from(x) * i64::from(window.w) / 1000;
    let py = i64::from(window.y) + i64::from(y) * i64::from(window.h) / 1000;
    let max_x = window.right().saturating_sub(1).max(i64::from(window.x));
    let max_y = window.bottom().saturating_sub(1).max(i64::from(window.y));
    (
        i32::try_from(px.clamp(i64::from(window.x), max_x)).unwrap_or(window.x),
        i32::try_from(py.clamp(i64::from(window.y), max_y)).unwrap_or(window.y),
    )
}

/// Smallest enabled control under a screen point, ignoring the window-sized pane.
pub(crate) fn control_under(snap: &UiSnapshot, at: (i32, i32)) -> Option<&UiElement> {
    let win = snap.window.rect.area().max(1);
    let p = Point::new(at.0, at.1);
    snap.elements
        .iter()
        .filter(|e| e.enabled && !e.rect.is_empty() && e.rect.contains(p))
        .filter(|e| e.rect.area().saturating_mul(10) < win.saturating_mul(7))
        .min_by_key(|e| e.rect.area())
}

pub(crate) fn selector_of(e: &UiElement) -> Selector {
    Selector {
        role: e.role.clone(),
        name: e.name.clone(),
        automation_id: e.automation_id.clone(),
        rect: Some(e.rect),
        ..Selector::default()
    }
}

/// Where a picture-point should be clicked: the control under it, or the point itself.
pub(crate) fn aim(snap: &UiSnapshot, x: i32, y: i32) -> Result<Selector, String> {
    if !(0..=1000).contains(&x) || !(0..=1000).contains(&y) {
        return Err("x and y must be from 0 to 1000 inside the picture, not screen pixels".into());
    }
    if snap.window.rect.is_empty() {
        return Err("the window has no size to point at".into());
    }
    let at = picture_point(snap.window.rect, x, y);
    if let Some(e) = control_under(snap, at) {
        return Ok(selector_of(e));
    }
    Ok(Selector {
        rect: Some(Rect::new(
            at.0.saturating_sub(4),
            at.1.saturating_sub(4),
            8,
            8,
        )),
        ..Selector::default()
    })
}

/// Control list in picture coordinates, so a model can point even when the picture is small.
pub(crate) fn control_lines(snap: &UiSnapshot, max_lines: usize, max_chars: usize) -> String {
    let win = snap.window.rect;
    let mut out = format!(
        "window \"{}\" (point with x,y from 0 to 1000)\n",
        clip(&snap.window.title, 80)
    );
    let mut n = 0;
    for e in &snap.elements {
        if e.rect.is_empty() || (e.name.is_empty() && e.automation_id.is_empty()) {
            continue;
        }
        if n >= max_lines || out.len() >= max_chars {
            out.push_str("...\n");
            break;
        }
        n += 1;
        let (cx, cy) = center(e.rect);
        let x = norm(cx, win.x, win.w);
        let y = norm(cy, win.y, win.h);
        out.push_str(&format!("#{} {}", e.id, e.role));
        if !e.name.is_empty() {
            out.push_str(&format!(" \"{}\"", clip(&e.name, 40)));
        }
        if !e.enabled {
            out.push_str(" disabled");
        }
        out.push_str(&format!(" at x={x} y={y}\n"));
    }
    out
}

fn norm(center: i32, origin: i32, span: u32) -> i32 {
    if span == 0 {
        return 0;
    }
    let v = (i64::from(center.saturating_sub(origin)) * 1000) / i64::from(span);
    i32::try_from(v.clamp(0, 1000)).unwrap_or(0)
}
