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

    /// The platform returned a non-2xx status (other than 401, which has
    /// its own [`Error::Unauthorized`] variant). `body` is truncated to a
    /// reasonable size before being attached.
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

pub type Result<T> = std::result::Result<T, Error>;
