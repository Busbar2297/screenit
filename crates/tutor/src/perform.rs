//! Performance: replay a rehearsed script at human pace while a recorder captures each chapter.
//! No language model is involved, so timing is steady and the video is clean.

use crate::error::{TutorError, TutorResult};
use crate::player::{Marker, Mode, Player};
use crate::safety::Guard;
use crate::script::TutorialScript;
use crate::traits::{Rig, WindowInfo};
use screenit_geom::Rect;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One finished chapter video.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub path: PathBuf,
    pub duration_ms: u64,
}

pub trait ChapterRecorder {
    fn start(&mut self, chapter: usize, region: Rect) -> TutorResult<()>;
    fn stop(&mut self) -> TutorResult<Segment>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Take {
    pub chapter: usize,
    pub title: String,
    pub path: PathBuf,
    pub duration_ms: u64,
    pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Performance {
    pub title: String,
    /// Screen area every chapter was recorded from.
    pub region: Rect,
    pub takes: Vec<Take>,
}

pub const LEAD_IN_MS: u32 = 700;
pub const TAIL_MS: u32 = 900;

/// Recording area for a window: its rectangle with even sides, at least 64 px.
pub fn region_for(w: Rect) -> Rect {
    Rect::new(w.x, w.y, (w.w & !1).max(64), (w.h & !1).max(64))
}

/// Play `chapters` (all when `None`) and append the takes to `out`.
/// What a performance runs against.
pub struct Stage<'a> {
    pub rig: &'a Rig,
    pub guard: Guard,
    pub window: WindowInfo,
}

pub fn perform(
    script: &TutorialScript,
    stage: Stage<'_>,
    recorder: &mut dyn ChapterRecorder,
    chapters: Option<&[usize]>,
    out: &mut Performance,
    on_chapter: &mut dyn FnMut(usize),
) -> TutorResult<()> {
    let Stage { rig, guard, window } = stage;
    let region = region_for(window.rect);
    out.title = script.title.clone();
    out.region = region;
    let mut player = Player::new(rig, guard, window, Mode::Perform);
    for (ci, chapter) in script.chapters.iter().enumerate() {
        if chapters.is_some_and(|c| !c.contains(&ci)) {
            continue;
        }
        if !chapter.checked {
            return Err(TutorError::Script(format!(
                "chapter {} was not rehearsed successfully; run tutor.rehearse first",
                ci + 1
            )));
        }
        on_chapter(ci);
        recorder.start(ci, region)?;
        player.begin_chapter(ci);
        let res = (|| {
            rig.pace.sleep_ms(LEAD_IN_MS);
            let mut markers = Vec::with_capacity(chapter.steps.len());
            for step in &chapter.steps {
                markers.push(player.run_step(step)?);
            }
            rig.pace.sleep_ms(TAIL_MS);
            Ok::<_, TutorError>(markers)
        })();
        let stopped = recorder.stop();
        let markers = res?;
        let seg = stopped?;
        out.takes.retain(|t| t.chapter != ci);
        out.takes.push(Take {
            chapter: ci,
            title: chapter.title.clone(),
            path: seg.path,
            duration_ms: seg.duration_ms,
            markers,
        });
    }
    out.takes.sort_by_key(|t| t.chapter);
    Ok(())
}
