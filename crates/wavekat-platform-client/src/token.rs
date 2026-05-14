//! Newtype wrapper around the platform bearer token.
//!
//! Why a newtype rather than `String`?
//!
//! - **Redacted `Debug`.** `format!("{t:?}")` won't leak the secret into
//!   logs or panic messages. The CLI bit us once where a `dbg!(cfg)`
//!   spilled the full token into a stderr line that ended up in a
//!   support transcript.
//! - **No `Display`.** Callers must explicitly opt in via [`Token::as_str`]
//!   to get the raw string, which makes accidental `format!("{t}")` (or
//!   `println!("{t}")`) a compile error.
//! - **Storage-agnostic.** Construction from any `Into<String>` so
//!   consumers can hand in whatever shape their storage layer returns.

use std::fmt;

/// Bearer token used to authenticate against the platform.
///
/// Tokens are minted by completing the loopback OAuth handshake (see
/// [`crate::oauth::loopback_handshake`]) or accepted out-of-band by the
/// caller (e.g. read from an environment variable in CI).
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Wrap a raw token string.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The raw token. Hand to HTTP code that needs to set the bearer
    /// header. Avoid logging this — that's the whole reason `Display`
    /// isn't implemented.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Show the prefix up to (and including) the underscore — which
        // is conventional for these tokens (`wk_…`) and useful for
        // quickly distinguishing "I have a token" from "I don't" in
        // logs without exposing any of the secret bytes.
        let prefix = match self.0.split_once('_') {
            Some((p, _)) => p,
            None => "",
        };
        if prefix.is_empty() {
            write!(f, "Token(***redacted)")
        } else {
            write!(f, "Token({prefix}_***redacted)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secret() {
        let secret = "wk_abcdef1234567890";
        let t = Token::new(secret);
        let dbg = format!("{t:?}");
        assert!(dbg.contains("***"), "{dbg} should contain ***");
        assert!(!dbg.contains(secret), "{dbg} leaked the secret");
        assert!(dbg.contains("wk_"), "{dbg} should keep the prefix");
    }

    #[test]
    fn debug_handles_no_prefix() {
        let t = Token::new("rawvalue");
        let dbg = format!("{t:?}");
        assert!(!dbg.contains("rawvalue"));
        assert!(dbg.contains("***"));
    }

    #[test]
    fn as_str_returns_raw() {
        let t = Token::new("wk_xyz");
        assert_eq!(t.as_str(), "wk_xyz");
    }
}
