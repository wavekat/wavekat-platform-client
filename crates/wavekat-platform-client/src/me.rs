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
    /// Opaque platform user id. A UUID string since wavekat-platform
    /// switched `users.id` off integer surrogate keys (which leaked the
    /// signup count). Treat as an opaque token — never parse or compare
    /// numerically.
    pub id: String,
    pub login: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: String,
}

/// Counts of what deleting the signed-in account would take, as
/// returned by `GET /api/me/deletion-preview`.
///
/// Public for the same reason [`Me`] is: every consumer shows these
/// before asking for confirmation, so nobody confirms an irreversible
/// action without seeing what is in it. Counts are of the account's
/// *own* content — anything it authored inside someone else's project
/// is not counted, because the platform's purge will not take it.
#[derive(Debug, Clone, Deserialize)]
pub struct DeletionPreview {
    pub calls: u32,
    pub recordings: u32,
    pub transcripts: u32,
    /// Share links that still resolve. Already-revoked ones are not
    /// counted — they are gone as far as the holder of the link is
    /// concerned.
    pub shares: u32,
    pub flows: u32,
    pub contacts: u32,
    pub prompts: u32,
    pub projects: u32,
    pub files: u32,
    pub annotations: u32,
    pub exports: u32,
    pub models: u32,
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

    /// Fetch what deleting this account would take, for the
    /// confirmation a consumer shows before calling
    /// [`Client::delete_account`].
    pub async fn deletion_preview(&self) -> Result<DeletionPreview> {
        self.get_json("/api/me/deletion-preview").await
    }

    /// Delete the signed-in account.
    ///
    /// Credentials, live share links and the profile go immediately and
    /// the account stops working at once; the remaining content and
    /// stored audio are purged by the platform within 30 days. There is
    /// no undo and no cancel, and signing in again with the same
    /// provider identity is refused until that purge completes.
    ///
    /// Requires a credential minted in the last few minutes — otherwise
    /// this returns [`Error::ReauthRequired`], and the caller should
    /// sign in again and retry rather than treat the token as dead.
    /// Once it succeeds, the token this client holds is gone: drop the
    /// `Client` and clear whatever storage held it.
    ///
    /// [`Error::ReauthRequired`]: crate::Error::ReauthRequired
    pub async fn delete_account(&self) -> Result<()> {
        self.delete("/api/me").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_preview_reads_the_platform_wire_shape() {
        // Verbatim from `GET /api/me/deletion-preview`. The desktop
        // dialog renders these counts before an irreversible action, so
        // a field renamed on the platform has to fail here rather than
        // silently show a zero.
        let json = r#"{
            "accounts": 2, "calls": 6, "recordings": 1, "transcripts": 4,
            "shares": 0, "flows": 3, "contacts": 9, "prompts": 2,
            "projects": 1, "labelSets": 0, "files": 5, "annotations": 90,
            "exports": 1, "models": 1
        }"#;
        let p: DeletionPreview = serde_json::from_str(json).unwrap();
        assert_eq!(p.calls, 6);
        assert_eq!(p.recordings, 1);
        assert_eq!(p.annotations, 90);
        assert_eq!(p.models, 1);
    }

    #[test]
    fn deletion_preview_needs_every_count() {
        // Missing fields must be an error, not a default zero: "nothing
        // will be deleted" is the one wrong answer this dialog can give.
        let err = serde_json::from_str::<DeletionPreview>(r#"{"calls": 6}"#);
        assert!(err.is_err(), "expected a decode error, got {err:?}");
    }
}
