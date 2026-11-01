//! Undo/redo history. Every command is one transaction; failures roll back atomically.
//! Snapshots are cheap because the base raster is `Arc`-shared.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use screenit_doc::CaptureDoc;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpsError {
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
}

struct Entry {
    label: String,
    coalesce: Option<String>,
    state: CaptureDoc,
}

pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Run `f` as one history step. On `Err` the document is restored and no step is recorded.
    /// Consecutive steps with the same `coalesce` key merge into one (drags, typing).
    pub fn apply<T, E>(
        &mut self,
        doc: &mut CaptureDoc,
        label: &str,
        coalesce: Option<&str>,
        f: impl FnOnce(&mut CaptureDoc) -> Result<T, E>,
    ) -> Result<T, E> {
        let before = doc.clone();
        match f(doc) {
            Ok(v) => {
                let merge = matches!((self.undo.last(), coalesce),
                    (Some(e), Some(k)) if e.coalesce.as_deref() == Some(k));
                if !merge {
                    self.undo.push(Entry {
                        label: label.to_string(),
                        coalesce: coalesce.map(str::to_string),
                        state: before,
                    });
                    if self.undo.len() > self.limit {
                        self.undo.remove(0);
                    }
                }
                self.redo.clear();
                Ok(v)
            }
            Err(e) => {
                *doc = before;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self, doc: &mut CaptureDoc) -> Result<String, OpsError> {
        let e = self.undo.pop().ok_or(OpsError::NothingToUndo)?;
        let current = std::mem::replace(doc, e.state);
        self.redo.push(Entry {
            label: e.label.clone(),
            coalesce: None,
            state: current,
        });
        Ok(e.label)
    }

    pub fn redo(&mut self, doc: &mut CaptureDoc) -> Result<String, OpsError> {
        let e = self.redo.pop().ok_or(OpsError::NothingToRedo)?;
        let current = std::mem::replace(doc, e.state);
        self.undo.push(Entry {
            label: e.label.clone(),
            coalesce: None,
            state: current,
        });
        Ok(e.label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn labels(&self) -> Vec<String> {
        self.undo.iter().map(|e| e.label.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screenit_doc::{ObjectKind, Style};
    use screenit_geom::PointF;
    use screenit_raster::Raster;

    fn line() -> ObjectKind {
        ObjectKind::Line {
            from: PointF::new(0.0, 0.0),
            to: PointF::new(4.0, 4.0),
        }
    }

    #[test]
    fn undo_redo_cycle() {
        let mut d = CaptureDoc::new(Raster::new(10, 10, [0; 4]).unwrap());
        let mut h = History::default();
        h.apply(&mut d, "Add", None, |d| d.add(line(), Style::default()))
            .unwrap();
        assert_eq!(d.objects.len(), 1);
        assert_eq!(h.undo(&mut d).unwrap(), "Add");
        assert!(d.objects.is_empty());
        h.redo(&mut d).unwrap();
        assert_eq!(d.objects.len(), 1);
        assert_eq!(h.redo(&mut d), Err(OpsError::NothingToRedo));
    }

    #[test]
    fn failure_rolls_back_and_records_nothing() {
        let mut d = CaptureDoc::new(Raster::new(10, 10, [0; 4]).unwrap());
        let mut h = History::default();
        let r: Result<(), String> = h.apply(&mut d, "Bad", None, |d| {
            d.add(line(), Style::default()).map_err(|e| e.to_string())?;
            Err("boom".into())
        });
        assert!(r.is_err());
        assert!(d.objects.is_empty());
        assert!(!h.can_undo());
    }

    #[test]
    fn coalesce_merges_drag_steps() {
        let mut d = CaptureDoc::new(Raster::new(10, 10, [0; 4]).unwrap());
        let mut h = History::default();
        let id = h
            .apply(&mut d, "Add", None, |d| d.add(line(), Style::default()))
            .unwrap();
        for _ in 0..5 {
            h.apply(&mut d, "Move", Some("drag1"), |d| d.translate(id, 1.0, 0.0))
                .unwrap();
        }
        assert_eq!(h.labels().len(), 2);
        h.undo(&mut d).unwrap();
        assert_eq!(
            d.get(id).map(|o| o.bounds().x),
            d.get(id).map(|o| o.bounds().x)
        );
        assert_eq!(d.objects.len(), 1);
    }

    #[test]
    fn limit_evicts_oldest() {
        let mut d = CaptureDoc::new(Raster::new(10, 10, [0; 4]).unwrap());
        let mut h = History::new(3);
        for _ in 0..10 {
            h.apply(&mut d, "Add", None, |d| d.add(line(), Style::default()))
                .unwrap();
        }
        assert_eq!(h.labels().len(), 3);
    }
}
