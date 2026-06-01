//! Ed25519 release-attestation signing for public (unauthenticated)
//! platform endpoints.
//!
//! Some platform endpoints run *before* any sign-in — the anonymous
//! install heartbeat (`POST /api/voice/installs/heartbeat`) is the first.
//! They carry no bearer token, so without another check anyone could
//! forge requests (e.g. spray fabricated `installId`s to inflate counts).
//! This module is the general signing primitive any such endpoint reuses
//! — it is not heartbeat-specific.
//!
//! ## The scheme: a tiny release certificate chain
//!
//! There is one offline **release master** Ed25519 keypair. Its private
//! half lives only in CI secrets and never touches the platform; its
//! public half is the platform's single root of trust.
//!
//! For each release, CI issues a short **certificate**: it generates a
//! fresh per-version keypair `(priv_v, pub_v)` and signs
//! `cert = Sign(masterPriv, cert_payload(version, pub_v))`. The build
//! then ships `priv_v`, `pub_v`, and `cert` (a [`ReleaseCredential`]).
//!
//! At request time the build signs a canonical description of the request
//! with `priv_v`, and sends `version`, `pub_v`, `cert`, and the request
//! signature in headers. The platform verifies with **only the master
//! public key**:
//!
//!   1. `cert` proves *we* blessed `pub_v` for `version` (so `pub_v` is
//!      trustworthy without the platform holding any secret), and
//!   2. the request signature proves the caller holds `priv_v`.
//!
//! ## Why asymmetric
//!
//! The platform stores no secret — only a public key — so a platform
//! breach cannot forge requests. The master private key never leaves CI.
//! The honest limit: `priv_v` is inside the distributed binary, so a
//! determined attacker can extract it. That compromises only *that
//! version* (not the master, not other versions), and the platform can
//! revoke it (min-version floor or denylist) without re-signing anything
//! else. So this buys casual-forgery resistance, blast-radius
//! containment, and revocability — not unbreakable auth.
//!
//! ## Wire layout (scheme version 2) — cross-repo contract
//!
//! Mirror these byte layouts exactly on the platform verifier; a change
//! here is a breaking protocol change and must bump [`SIG_VERSION`].
//!
//! Request canonical string the per-version key signs (six newline-joined
//! lines):
//!
//! ```text
//! WKSIG2
//! {unix_timestamp_secs}
//! {nonce_hex}
//! {METHOD}             // upper-case, e.g. POST
//! {path}              // request path, no host, no query
//! {sha256_hex(body)}  // lower-case hex of the exact body bytes sent
//! ```
//!
//! Certificate payload the master key signs (three newline-joined lines):
//!
//! ```text
//! WKCERT2
//! {version}           // the build version this key is issued for
//! {pub_v_hex}         // 64 hex chars (32-byte Ed25519 public key)
//! ```
//!
//! All keys/signatures travel as lower-case hex: keys 64 chars (32 bytes),
//! signatures 128 chars (64 bytes).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Signature scheme version. Bumping it (and the `WKSIG`/`WKCERT`
/// prefixes) lets the platform support old and new clients across a
/// transition.
pub(crate) const SIG_VERSION: &str = "2";

/// HTTP header carrying the scheme version (`"2"`).
pub(crate) const HEADER_VERSION: &str = "X-WK-Sig-Ver";
/// HTTP header carrying the unix-seconds timestamp the signature covers.
pub(crate) const HEADER_TIMESTAMP: &str = "X-WK-Sig-Ts";
/// HTTP header carrying the per-request nonce (hex).
pub(crate) const HEADER_NONCE: &str = "X-WK-Sig-Nonce";
/// HTTP header carrying the build version the per-version key is bound to.
pub(crate) const HEADER_BUILD_VERSION: &str = "X-WK-Build-Version";
/// HTTP header carrying the per-version Ed25519 public key (hex).
pub(crate) const HEADER_PUBKEY: &str = "X-WK-Pub";
/// HTTP header carrying the master-signed certificate over the pubkey (hex).
pub(crate) const HEADER_CERT: &str = "X-WK-Cert";
/// HTTP header carrying the per-version request signature (hex).
pub(crate) const HEADER_SIGNATURE: &str = "X-WK-Sig";

/// Everything a release build needs to sign requests: the per-version
/// private key, its public key, the master-issued certificate over that
/// public key, and the version the certificate is bound to.
///
/// CI produces these per release via [`issue_release_credential`] and
/// bakes them into the build (the private key as a compile-time secret;
/// the public key and cert are not sensitive). Construct one from those
/// baked strings and hand it to [`crate::Client::post_public_signed_json`].
#[derive(Debug, Clone)]
pub struct ReleaseCredential {
    /// 64-char hex of the 32-byte per-version Ed25519 private (seed) key.
    pub private_key_hex: String,
    /// 64-char hex of the 32-byte per-version Ed25519 public key.
    pub public_key_hex: String,
    /// 128-char hex of the 64-byte master signature over
    /// [`cert_payload`]`(version, public_key)`.
    pub cert_hex: String,
    /// The build version the certificate is bound to. Must match what the
    /// platform sees; the signer sends it in [`HEADER_BUILD_VERSION`].
    pub version: String,
}

/// The freshness/identity values produced when signing a request:
/// the unix-seconds timestamp and per-request nonce the signature covers,
/// plus the hex Ed25519 signature itself. Returned by
/// [`ReleaseCredential::sign_request`]; the caller sends these in the
/// `X-WK-Sig-Ts` / `X-WK-Sig-Nonce` / `X-WK-Sig` headers.
#[derive(Debug, Clone)]
pub struct RequestSignature {
    pub timestamp: String,
    pub nonce: String,
    pub signature_hex: String,
}

/// Lower-case hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// The canonical string a per-version key signs for a request. Kept
/// standalone so tests pin the exact byte layout the platform mirrors.
pub(crate) fn canonical_request(
    timestamp: &str,
    nonce: &str,
    method: &str,
    path: &str,
    body_hash_hex: &str,
) -> String {
    format!("WKSIG{SIG_VERSION}\n{timestamp}\n{nonce}\n{method}\n{path}\n{body_hash_hex}")
}

/// The certificate payload the master key signs to bless a per-version
/// public key. Binding the version in means a cert can't be reused to
/// vouch for a key under a different (e.g. un-revoked) version.
pub fn cert_payload(version: &str, public_key_hex: &str) -> String {
    format!("WKCERT{SIG_VERSION}\n{version}\n{public_key_hex}")
}

/// Current unix time in whole seconds. Falls back to 0 if the clock is
/// before the epoch — the platform's freshness window then rejects it,
/// which is the safe outcome.
pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 16 random bytes as lower-case hex (32 chars) for the per-request nonce.
pub(crate) fn random_nonce() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Decode a hex string into exactly `N` bytes, mapping any error to a
/// `BadRequest` with `what` for context.
fn hex_to_array<const N: usize>(hex_str: &str, what: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| Error::BadRequest(format!("{what}: invalid hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| Error::BadRequest(format!("{what}: expected {N} bytes")))
}

impl ReleaseCredential {
    /// Parse the per-version private key, returning the dalek signing key.
    fn signing_key(&self) -> Result<SigningKey> {
        let seed = hex_to_array::<32>(&self.private_key_hex, "release private key")?;
        Ok(SigningKey::from_bytes(&seed))
    }

    /// Sign a request's canonical string with the per-version key,
    /// producing the timestamp/nonce/signature to send. `body` is the
    /// exact bytes that will be sent as the request body; `method` is the
    /// upper-case HTTP verb and `path` the request path (no host, no
    /// query). [`Client::post_public_signed_json`] wraps this for the
    /// common JSON-POST case.
    pub fn sign_request(&self, method: &str, path: &str, body: &[u8]) -> Result<RequestSignature> {
        let signing_key = self.signing_key()?;
        let timestamp = unix_secs().to_string();
        let nonce = random_nonce();
        let body_hash = sha256_hex(body);
        let message = canonical_request(&timestamp, &nonce, method, path, &body_hash);
        let sig: Signature = signing_key.sign(message.as_bytes());
        Ok(RequestSignature {
            timestamp,
            nonce,
            signature_hex: hex::encode(sig.to_bytes()),
        })
    }
}

// ---- Release-key issuance (CI / tooling side) -----------------------------
//
// These run offline in CI, not in the daemon, but live here so the byte
// formats they emit are guaranteed to match what the signer above and the
// platform verifier consume. The `release-keys` binary (feature
// `release-tooling`) wraps them in a CLI.

/// A freshly generated Ed25519 keypair as hex `(private_hex, public_hex)`.
pub fn generate_keypair() -> (String, String) {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    (hex::encode(seed), hex::encode(verifying_key.to_bytes()))
}

/// Generate the offline release master keypair. Identical to
/// [`generate_keypair`] — named for intent at the call site (the master
/// is generated once, by hand, and its private half guarded as a CI
/// secret).
pub fn generate_master() -> (String, String) {
    generate_keypair()
}

/// Issue a [`ReleaseCredential`] for `version`: generate a per-version
/// keypair and sign its certificate with the master private key
/// (`master_private_key_hex`). Run by CI at release time.
pub fn issue_release_credential(
    master_private_key_hex: &str,
    version: &str,
) -> Result<ReleaseCredential> {
    let master_seed = hex_to_array::<32>(master_private_key_hex, "master private key")?;
    let master = SigningKey::from_bytes(&master_seed);
    let (private_key_hex, public_key_hex) = generate_keypair();
    let payload = cert_payload(version, &public_key_hex);
    let cert: Signature = master.sign(payload.as_bytes());
    Ok(ReleaseCredential {
        private_key_hex,
        public_key_hex,
        cert_hex: hex::encode(cert.to_bytes()),
        version: version.to_string(),
    })
}

// ---- Verification helpers (mirrors the platform; used by tests) -----------

/// Verify that `cert_hex` is a valid master signature over
/// `cert_payload(version, public_key_hex)`. The platform performs the
/// equivalent check in TypeScript; this Rust copy backs the round-trip
/// tests and documents the exact bytes.
pub fn verify_cert(
    master_public_key_hex: &str,
    version: &str,
    public_key_hex: &str,
    cert_hex: &str,
) -> Result<bool> {
    let master_pub = hex_to_array::<32>(master_public_key_hex, "master public key")?;
    let master = VerifyingKey::from_bytes(&master_pub)
        .map_err(|e| Error::BadRequest(format!("master public key: {e}")))?;
    let cert_bytes = hex_to_array::<64>(cert_hex, "certificate")?;
    let cert = Signature::from_bytes(&cert_bytes);
    let payload = cert_payload(version, public_key_hex);
    Ok(master.verify(payload.as_bytes(), &cert).is_ok())
}

/// Verify a request signature against a per-version public key.
pub fn verify_request(
    public_key_hex: &str,
    timestamp: &str,
    nonce: &str,
    method: &str,
    path: &str,
    body: &[u8],
    signature_hex: &str,
) -> Result<bool> {
    let pub_bytes = hex_to_array::<32>(public_key_hex, "public key")?;
    let verifying_key = VerifyingKey::from_bytes(&pub_bytes)
        .map_err(|e| Error::BadRequest(format!("public key: {e}")))?;
    let sig_bytes = hex_to_array::<64>(signature_hex, "signature")?;
    let sig = Signature::from_bytes(&sig_bytes);
    let body_hash = sha256_hex(body);
    let message = canonical_request(timestamp, nonce, method, path, &body_hash);
    Ok(verifying_key.verify(message.as_bytes(), &sig).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_request_layout_is_stable() {
        // Cross-repo wire contract — the platform verifier re-derives this
        // exact six-line layout.
        let s = canonical_request(
            "1700000000",
            "deadbeef",
            "POST",
            "/api/voice/installs/heartbeat",
            "abc123",
        );
        assert_eq!(
            s,
            "WKSIG2\n1700000000\ndeadbeef\nPOST\n/api/voice/installs/heartbeat\nabc123"
        );
    }

    #[test]
    fn cert_payload_layout_is_stable() {
        let s = cert_payload("0.0.22", "ab12");
        assert_eq!(s, "WKCERT2\n0.0.22\nab12");
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn generate_keypair_yields_distinct_32_byte_keys() {
        let (priv_a, pub_a) = generate_keypair();
        let (priv_b, _pub_b) = generate_keypair();
        assert_eq!(priv_a.len(), 64, "32-byte private → 64 hex chars");
        assert_eq!(pub_a.len(), 64, "32-byte public → 64 hex chars");
        assert_ne!(priv_a, priv_b, "fresh keypairs must differ");
    }

    #[test]
    fn full_chain_round_trips() {
        // The whole flow the way CI + a build + the platform run it.
        let (master_priv, master_pub) = generate_master();
        let cred = issue_release_credential(&master_priv, "0.0.22").unwrap();

        // Platform step 1: the cert blesses pub_v for this version.
        assert!(
            verify_cert(&master_pub, "0.0.22", &cred.public_key_hex, &cred.cert_hex).unwrap(),
            "valid cert must verify under the master public key"
        );

        // Build signs a request; platform step 2 verifies it with pub_v.
        let body = br#"{"installId":"x"}"#;
        let rs = cred
            .sign_request("POST", "/api/voice/installs/heartbeat", body)
            .unwrap();
        assert!(
            verify_request(
                &cred.public_key_hex,
                &rs.timestamp,
                &rs.nonce,
                "POST",
                "/api/voice/installs/heartbeat",
                body,
                &rs.signature_hex,
            )
            .unwrap(),
            "a freshly signed request must verify under its pub_v"
        );
    }

    #[test]
    fn cert_is_rejected_under_the_wrong_master() {
        let (master_priv, _master_pub) = generate_master();
        let (_other_priv, other_pub) = generate_master();
        let cred = issue_release_credential(&master_priv, "0.0.22").unwrap();
        // A different master's public key must not validate this cert.
        assert!(
            !verify_cert(&other_pub, "0.0.22", &cred.public_key_hex, &cred.cert_hex).unwrap(),
            "cert must not verify under a foreign master key"
        );
    }

    #[test]
    fn cert_is_bound_to_its_version() {
        let (master_priv, master_pub) = generate_master();
        let cred = issue_release_credential(&master_priv, "0.0.22").unwrap();
        // Same pub key, different claimed version → cert no longer matches,
        // so an attacker can't reuse a cert to dodge a version denylist.
        assert!(
            !verify_cert(&master_pub, "0.0.99", &cred.public_key_hex, &cred.cert_hex).unwrap(),
            "cert must not verify for a different version"
        );
    }

    #[test]
    fn tampered_request_body_fails_verification() {
        let (master_priv, _master_pub) = generate_master();
        let cred = issue_release_credential(&master_priv, "0.0.22").unwrap();
        let rs = cred.sign_request("POST", "/p", b"original").unwrap();
        assert!(
            !verify_request(
                &cred.public_key_hex,
                &rs.timestamp,
                &rs.nonce,
                "POST",
                "/p",
                b"tampered",
                &rs.signature_hex,
            )
            .unwrap(),
            "a body that differs from the signed one must fail"
        );
    }

    #[test]
    fn request_signed_by_one_version_fails_under_another_pubkey() {
        let (master_priv, _master_pub) = generate_master();
        let cred_a = issue_release_credential(&master_priv, "0.0.22").unwrap();
        let cred_b = issue_release_credential(&master_priv, "0.0.23").unwrap();
        let rs = cred_a.sign_request("POST", "/p", b"body").unwrap();
        // Verifying A's signature with B's public key must fail.
        assert!(
            !verify_request(
                &cred_b.public_key_hex,
                &rs.timestamp,
                &rs.nonce,
                "POST",
                "/p",
                b"body",
                &rs.signature_hex,
            )
            .unwrap(),
            "a signature must only verify under its own pub_v"
        );
    }
}
