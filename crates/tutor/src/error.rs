#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TutorError {
    #[error("ui automation: {0}")]
    Ui(String),
    #[error("input: {0}")]
    Input(String),
    #[error("launch: {0}")]
    Launch(String),
    #[error("language model: {0}")]
    Llm(String),
    #[error("script: {0}")]
    Script(String),
    /// A safety rule stopped the run (wrong window, password field, budget, confirmation).
    #[error("safety: {0}")]
    Safety(String),
    #[error("aborted: {0}")]
    Aborted(String),
    #[error("step failed: {0}")]
    Step(String),
    #[error("recording: {0}")]
    Record(String),
}

pub type TutorResult<T> = Result<T, TutorError>;
