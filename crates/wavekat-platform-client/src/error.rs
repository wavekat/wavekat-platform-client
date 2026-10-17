//! Public error type for the crate.
//!
//! Library convention: typed variants so consumers can `match` on the
//! failure mode (network vs. HTTP status vs. OAuth state mismatch).
//! End-user binaries can `?` these into their own `anyhow::Result`
//! without losing information.

use std::time::Duration;

/// All errors surfaced by the crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The platform returned 401. Split out from [`Error::Http`] so
    /// consumers can render a tailored "sign in again" message instead
    /// of the raw response body — the right remedy is almost always
    /// "mint a fresh token", and the body alone (`{"error":"unauthenticated"}`)
    /// doesn't tell the user that.
    #[error("HTTP 401 {url}: {body}")]
    Unauthorized { url: String, body: String },

    /// The platform returned 401 `reauth_required`: the credential is
    /// valid but was not minted recently enough for a destructive
    /// action. Split out from [`Error::Unauthorized`] because the
    /// remedy is different — the caller signs in again and retries,
    /// rather than treating the session as dead and dropping the token.
    #[error("re-authentication required: {url}")]
    ReauthRequired { url: String },

    /// The platform returned a non-2xx status (other than 401, which
    /// splits into [`Error::Unauthorized`] and [`Error::ReauthRequired`]).
    /// `body` is truncated to a reasonable size before being attached.
    #[error("HTTP {status} {url}: {body}")]
    Http {
        status: u16,
        url: String,
        body: String,
    },

    /// Underlying transport failure (DNS, TLS, connection reset, …).
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// The response body wasn't valid JSON for the expected shape.
    #[error("decoding response from {url}: {source}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    /// The OAuth callback returned a `state` value that didn't match
    /// what we generated. Refusing the token is the only safe move.
    #[error("OAuth state mismatch — got {actual:?}, expected {expected:?}")]
    StateMismatch {
        actual: Option<String>,
        expected: String,
    },

    /// The user (or the platform) cancelled the OAuth flow in the
    /// browser. The `String` carries the platform-supplied reason.
    #[error("OAuth flow cancelled in browser: {0}")]
    Cancelled(String),

    /// The OAuth handshake didn't complete within the allotted time.
    #[error("OAuth handshake timed out after {0:?}")]
    Timeout(Duration),

    /// Caller-side problem — usually a malformed input (e.g. a token
    /// that contains bytes we can't put in an HTTP header).
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Local I/O failure (loopback bind, socket read/write, …).
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),

    /// The platform refused a voice-prompt (text-to-speech) request for a
    /// reason the caller can act on — see [`VoicePromptError`]. Split out
    /// from [`Error::Http`] so a consumer can say "quota used up, resets
    /// on …" instead of showing a raw `HTTP 429` body.
    #[error(transparent)]
    VoicePrompt(#[from] VoicePromptError),
}

/// Why `POST /api/voice/prompts/generate` refused to render a clip.
///
/// Each variant is one of the platform's machine-readable error codes,
/// and its `Display` is a plain sentence a consumer can show as-is. A
/// sign-in problem is not here: it surfaces as [`Error::Unauthorized`],
/// the same remedy as every other endpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VoicePromptError {
    /// The text (after any phrasing preset is applied) is longer than the
    /// per-request character cap. `max_chars` is the cap in force, when
    /// the client could read it back from `GET /api/voice/prompts/config`.
    #[error("{}", text_too_long_message(*.max_chars))]
    TextTooLong { max_chars: Option<u32> },

    /// The caller's character allowance is used up. `used` / `limit` are
    /// filled when the platform reports them; `resets_at` (RFC 3339) is
    /// when the allowance renews.
    #[error("{}", quota_message(*.kind, *.used, *.limit, .resets_at.as_deref()))]
    QuotaExceeded {
        kind: VoicePromptQuotaKind,
        used: Option<u32>,
        limit: Option<u32>,
        resets_at: Option<String>,
    },

    /// The voice id isn't one of the platform's offered voices (or is a
    /// retired one this caller never used).
    #[error("unknown text-to-speech voice {voice_id:?}")]
    UnknownVoice { voice_id: String },

    /// The upstream text-to-speech provider failed. Nothing was billed;
    /// retrying later is the remedy.
    #[error("the text-to-speech provider failed to render the prompt; try again later")]
    ProviderFailed,
}

/// Which allowance a [`VoicePromptError::QuotaExceeded`] ran out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VoicePromptQuotaKind {
    /// The signed-in caller's monthly character quota (`quota_exceeded`).
    Monthly,
    /// An anonymous caller's free daily allowance (`anon_daily_exceeded`).
    /// Seen only when the request reached the platform without a valid
    /// session.
    AnonDaily,
    /// The platform-wide anonymous budget for today (`anon_budget_exceeded`).
    AnonBudget,
}

fn text_too_long_message(max_chars: Option<u32>) -> String {
    match max_chars {
        Some(n) => format!("prompt text is too long: the limit is {n} characters"),
        None => "prompt text is too long for the platform's character limit".to_string(),
    }
}

fn quota_message(
    kind: VoicePromptQuotaKind,
    used: Option<u32>,
    limit: Option<u32>,
    resets_at: Option<&str>,
) -> String {
    let mut s = match kind {
        VoicePromptQuotaKind::Monthly => "monthly voice prompt quota used up",
        VoicePromptQuotaKind::AnonDaily => {
            "free daily voice prompt allowance used up (not signed in)"
        }
        VoicePromptQuotaKind::AnonBudget => {
            "today's free voice prompt budget is used up (not signed in)"
        }
    }
    .to_string();
    if let (Some(used), Some(limit)) = (used, limit) {
        s.push_str(&format!(" ({used} of {limit} characters)"));
    }
    if let Some(at) = resets_at {
        s.push_str(&format!("; resets at {at}"));
    }
    s
}

/// Split a 401 body into the two errors that need different remedies.
///
/// The platform answers `reauth_required` when the credential is fine
/// but too old to authorise something irreversible — the caller signs
/// in again and retries. Every other 401 means the credential itself is
/// finished, and retrying with it is pointless.
///
/// Matched as a substring rather than parsed as JSON on purpose: a 401
/// can also arrive from something in front of the API with an HTML
/// body, and this must never fail closed into the wrong remedy just
/// because the body didn't deserialize.
pub(crate) fn classify_unauthorized(url: String, body: String) -> Error {
    if body.contains("reauth_required") {
        Error::ReauthRequired { url }
    } else {
        Error::Unauthorized { url, body }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_401_naming_reauth_is_classified_as_reauth_required() {
        let err = classify_unauthorized(
            "https://api.test/api/me".to_string(),
            r#"{"error":"reauth_required"}"#.to_string(),
        );
        assert!(matches!(err, Error::ReauthRequired { .. }), "got {err:?}");
    }

    #[test]
    fn a_plain_401_stays_unauthorized() {
        let err = classify_unauthorized(
            "https://api.test/api/me".to_string(),
            r#"{"error":"unauthenticated"}"#.to_string(),
        );
        assert!(matches!(err, Error::Unauthorized { .. }), "got {err:?}");
    }

    #[test]
    fn reauth_required_says_so_and_names_the_url() {
        // The daemon logs this verbatim; "401" alone reads as a dead
        // session, which is the wrong remedy.
        let err = classify_unauthorized(
            "https://api.test/api/me".to_string(),
            r#"{"error":"reauth_required"}"#.to_string(),
        );
        let s = err.to_string();
        assert!(s.contains("re-authentication"), "{s}");
        assert!(s.contains("https://api.test/api/me"), "{s}");
    }
}
