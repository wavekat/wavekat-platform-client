//! HMAC-SHA256 request signing for public (unauthenticated) platform
//! endpoints.
//!
//! Some platform endpoints run *before* any sign-in — the anonymous
//! install heartbeat (`POST /api/voice/installs/heartbeat`) is the first
//! one. They carry no bearer token, so without some other check anyone
//! could forge requests (e.g. spray fake `installId`s to inflate counts).
//!
//! The protection is a shared-secret HMAC signature. The caller (a
//! genuine product build) holds a per-build secret injected at compile
//! time; the platform holds the same secret(s). The client signs a
//! canonical description of the request and ships the signature in
//! headers; the platform recomputes it and rejects a mismatch.
//!
//! ## Honest scope
//!
//! A secret baked into a distributed binary is *extractable* by a
//! determined attacker — this raises the bar against casual scraping and
//! lets us **rotate** the key per release to burn a leaked one, but it is
//! not unbreakable auth. The secret never travels on the wire (only an
//! HMAC over the request does), so passively sniffing a request doesn't
//! reveal it.
//!
//! ## Canonical string (version 1)
//!
//! Six newline-joined lines — this exact layout is the wire contract the
//! platform's verifier mirrors, so never reorder or reword it:
//!
//! ```text
//! WKHB1
//! {unix_timestamp_secs}
//! {nonce_hex}
//! {METHOD}            // upper-case, e.g. POST
//! {path}             // request path, no host, no query — e.g. /api/voice/installs/heartbeat
//! {sha256_hex(body)} // lower-case hex of the exact body bytes sent
//! ```
//!
//! The timestamp is covered by the signature, so the platform's freshness
//! window (it rejects timestamps outside a few minutes of its clock)
//! makes a captured request un-replayable once it expires. The nonce adds
//! per-request entropy and is reserved for an optional future single-use
//! check on the platform side.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Signature scheme version. Bumping it (and the `WKHB` prefix below)
/// lets the platform support old and new clients during a transition.
pub(crate) const SIG_VERSION: &str = "1";

/// HTTP header carrying the scheme version (`"1"`).
pub(crate) const HEADER_VERSION: &str = "X-WK-Sig-Ver";
/// HTTP header carrying the unix-seconds timestamp the signature covers.
pub(crate) const HEADER_TIMESTAMP: &str = "X-WK-Sig-Ts";
/// HTTP header carrying the per-request nonce (hex).
pub(crate) const HEADER_NONCE: &str = "X-WK-Sig-Nonce";
/// HTTP header carrying the lower-case hex HMAC-SHA256 signature.
pub(crate) const HEADER_SIGNATURE: &str = "X-WK-Sig";

/// Lower-case hex SHA-256 of `bytes`. Used both for the body hash inside
/// the canonical string and (on the platform) to re-derive it.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Build the canonical string the signature is computed over. Kept as a
/// standalone function (not inlined) so the unit tests can pin the exact
/// byte layout the platform verifier depends on.
pub(crate) fn canonical_string(
    timestamp: &str,
    nonce: &str,
    method: &str,
    path: &str,
    body_hash_hex: &str,
) -> String {
    format!("WKHB{SIG_VERSION}\n{timestamp}\n{nonce}\n{method}\n{path}\n{body_hash_hex}")
}

/// HMAC-SHA256 of `message` under `key`, lower-case hex. `key` is the
/// per-build shared secret as raw UTF-8 bytes.
pub(crate) fn hmac_hex(key: &[u8], message: &str) -> String {
    // `new_from_slice` only errors for key types with a fixed length
    // requirement; HMAC accepts any key length, so this never fails.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Current unix time in whole seconds. Falls back to 0 if the system
/// clock is somehow before the epoch — the platform's freshness window
/// then rejects the request, which is the safe outcome.
pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 16 random bytes as lower-case hex (32 chars). Entropy source for the
/// per-request nonce.
pub(crate) fn random_nonce() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// The four header values for a freshly-signed request: `(version,
/// timestamp, nonce, signature)`. `key` is the per-build secret; `method`
/// is the upper-case HTTP verb; `path` is the request path; `body` is the
/// exact bytes that will be sent as the request body.
pub(crate) fn sign_request(
    key: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> (String, String, String, String) {
    let timestamp = unix_secs().to_string();
    let nonce = random_nonce();
    let body_hash = sha256_hex(body);
    let message = canonical_string(&timestamp, &nonce, method, path, &body_hash);
    let signature = hmac_hex(key.as_bytes(), &message);
    (SIG_VERSION.to_string(), timestamp, nonce, signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_string_layout_is_stable() {
        // This exact six-line layout is the cross-repo wire contract the
        // platform verifier re-derives. A change here is a breaking
        // protocol change and must be matched on the platform side.
        let s = canonical_string(
            "1700000000",
            "deadbeef",
            "POST",
            "/api/voice/installs/heartbeat",
            "abc123",
        );
        assert_eq!(
            s,
            "WKHB1\n1700000000\ndeadbeef\nPOST\n/api/voice/installs/heartbeat\nabc123"
        );
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256 of the empty input — a fixed, widely-published vector.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hmac_hex_matches_rfc4231_test_case_2() {
        // RFC 4231 §4.3 test case 2: key="Jefe", data="what do ya want
        // for nothing?". Pins our HMAC-SHA256 to the standard so a dep
        // swap can't silently change the algorithm.
        let sig = hmac_hex(b"Jefe", "what do ya want for nothing?");
        assert_eq!(
            sig,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hmac_is_deterministic_and_key_sensitive() {
        let msg = canonical_string("1700000000", "n0nce", "POST", "/p", "bodyhash");
        let a = hmac_hex(b"key-one", &msg);
        let b = hmac_hex(b"key-one", &msg);
        let c = hmac_hex(b"key-two", &msg);
        assert_eq!(a, b, "same key + message must be deterministic");
        assert_ne!(a, c, "different key must change the signature");
    }

    #[test]
    fn sign_request_is_verifiable_by_recomputation() {
        // Simulate the platform: take the emitted headers, rebuild the
        // canonical string, recompute the HMAC, and confirm it matches.
        let key = "build-secret";
        let body = br#"{"installId":"x"}"#;
        let (ver, ts, nonce, sig) =
            sign_request(key, "POST", "/api/voice/installs/heartbeat", body);
        assert_eq!(ver, "1");
        let recomputed = hmac_hex(
            key.as_bytes(),
            &canonical_string(
                &ts,
                &nonce,
                "POST",
                "/api/voice/installs/heartbeat",
                &sha256_hex(body),
            ),
        );
        assert_eq!(sig, recomputed);
    }

    #[test]
    fn nonce_is_random_per_call() {
        assert_ne!(random_nonce(), random_nonce(), "nonces must differ");
        assert_eq!(random_nonce().len(), 32, "16 bytes → 32 hex chars");
    }
}
