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
