//! `/api/me` — the typed shape of the signed-in user.
//!
//! Public because every consumer needs it: the CLI prints it after
//! `wk login`/`wk me`, and the desktop daemon shows the same fields in
//! its Platform settings page. Keeping the struct here (and re-exported
//! from the crate root) means consumers don't redefine it.

use serde::Deserialize;

use crate::client::Client;
use crate::error::Result;

/// The signed-in user, as returned by `GET /api/me`.
#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: String,
}

impl Client {
    /// Fetch the signed-in user from `/api/me`. The canonical way to
    /// verify a freshly-minted token is reachable.
    pub async fn whoami(&self) -> Result<Me> {
        self.get_json("/api/me").await
    }

    /// Revoke the bearer token this client is using. After this returns
    /// successfully, the same token will start producing 401s — drop the
    /// `Client` (and clear whatever storage held the token).
    pub async fn revoke_current_token(&self) -> Result<()> {
        self.post_empty("/api/auth/cli/tokens/revoke-current").await
    }
}
