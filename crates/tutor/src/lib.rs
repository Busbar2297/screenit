//! Tutorial engine: a script of chapters and steps is rehearsed against a live app through UI
//! Automation (a language model repairs failing steps), then performed at human pace while a
//! recorder captures each chapter, then produced into one video with zoom, captions and chapters.
//!
//! Pure logic and trait seams; the Win32 side lives in `screenit-platform`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod error;
pub mod fakes;
pub mod keys;
pub mod path;
pub mod perform;
pub mod plan;
pub mod player;
pub mod produce;
pub mod rehearse;
pub mod safety;
pub mod script;
mod traits;

pub use error::{TutorError, TutorResult};
pub use traits::*;

#[cfg(test)]
mod tests;
