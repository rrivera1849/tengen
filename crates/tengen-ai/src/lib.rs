//! Everything Tengen does with KataGo and the LLM.
//!
//! - [`katago`]: runs KataGo's analysis engine and talks to it.
//! - [`runtime`]: finds or downloads KataGo and its network.
//! - [`estimate`]: the score estimate for one position.
//! - [`analysis`]: the two passes over a whole game.
//! - [`review`]: picking the lessons and building each one's fact sheet.
//! - [`llm`]: turning fact sheets into teaching, and checking the result.
//! - [`maxims`]: the player's maxims across games.
//!
//! The rule that runs through all of it: KataGo decides what is true; the
//! LLM only explains it.

pub mod analysis;
pub mod estimate;
pub mod katago;
pub mod llm;
pub mod maxims;
pub mod review;
pub mod runtime;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("KataGo isn't installed yet")]
    NotInstalled,
    #[error("KataGo couldn't start: {0}")]
    Start(String),
    #[error("KataGo stopped: {0}")]
    Engine(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("the review service said: {0}")]
    Llm(String),
    #[error("add a Claude API key to write lessons")]
    NoApiKey,
    #[error("{0}")]
    Other(String),
}
