//! Adapter for the Fox Go Server (野狐围棋, foxwq.com).
//!
//! Fox has no public API. This adapter will speak its protocol once it has
//! been decoded from captures of the official client. Until then every call
//! returns [`AdapterError::Unsupported`], so the rest of Tengen can already
//! list Fox as a server.
//!
//! Research plan (phase 4 of the roadmap):
//! 1. Run the official Windows client in a VM and capture its traffic.
//! 2. Decode framing, then login, then lobby and automatch messages.
//! 3. Keep each decoded message as a test fixture, so a client update that
//!    changes the protocol shows up as a failing test.
//!
//! Ground rules: behave like the official client, send nothing it would not,
//! and never automate play.

use tengen_core::adapter::{
    Account, AdapterError, Credentials, GameSummary, Server, ServerAdapter,
};

#[derive(Default)]
pub struct FoxAdapter;

impl ServerAdapter for FoxAdapter {
    fn server(&self) -> Server {
        Server::Fox
    }

    async fn sign_in(&mut self, _credentials: &Credentials) -> Result<Account, AdapterError> {
        Err(AdapterError::Unsupported)
    }

    async fn recent_games(&self, _player_id: &str) -> Result<Vec<GameSummary>, AdapterError> {
        Err(AdapterError::Unsupported)
    }

    async fn download_sgf(&self, _game_id: &str) -> Result<String, AdapterError> {
        Err(AdapterError::Unsupported)
    }
}
