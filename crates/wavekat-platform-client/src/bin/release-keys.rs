//! Release-key tooling for the Ed25519 attestation scheme (see
//! `src/sign.rs`). Two subcommands, both meant to run in CI / by hand —
//! never in the shipped daemon. Gated behind the `release-tooling`
//! feature so it isn't part of the crate's default build surface; install
//! it with:
//!
//! ```text
//! cargo install wavekat-platform-client --bin release-keys --features release-tooling
//! ```
//!
//! ## One-time setup
//!
//! ```text
//! release-keys master
//! ```
//!
//! Prints the offline release master keypair. Store the **private** line
//! as a CI secret (`WK_RELEASE_MASTER_PRIVATE`); publish/commit the
//! **public** line as the platform's `RELEASE_MASTER_PUBKEY`. The private
//! key never touches the platform.
//!
//! ## Per release (in CI, with the master private key in the env)
//!
//! ```text
//! WK_RELEASE_MASTER_PRIVATE=<hex> release-keys issue 0.0.22
//! ```
//!
//! Generates a fresh per-version keypair, signs its certificate with the
//! master key, and prints three `KEY=value` lines to export into the
//! build environment so they're baked in via `option_env!`:
//!
//! ```text
//! WK_RELEASE_PRIV_V=<hex>   # per-version private key — sensitive
//! WK_RELEASE_PUB_V=<hex>    # per-version public key — not sensitive
//! WK_RELEASE_CERT=<hex>     # master signature over (version ‖ pub_v)
//! ```
//!
//! The version passed here MUST equal the build's `CARGO_PKG_VERSION`, or
//! the daemon's signature (which sends `CARGO_PKG_VERSION`) won't match
//! the certificate and the platform will reject it.

use std::process::ExitCode;

use wavekat_platform_client::{generate_master, issue_release_credential};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("master") => {
            let (private_hex, public_hex) = generate_master();
            eprintln!("# Release master keypair — generated once.");
            eprintln!("# Store the private line as the CI secret WK_RELEASE_MASTER_PRIVATE.");
            eprintln!("# Set the public line as the platform var RELEASE_MASTER_PUBKEY.");
            println!("WK_RELEASE_MASTER_PRIVATE={private_hex}");
            println!("RELEASE_MASTER_PUBKEY={public_hex}");
            ExitCode::SUCCESS
        }
        Some("issue") => {
            let Some(version) = args.get(2) else {
                eprintln!("usage: release-keys issue <version>");
                return ExitCode::FAILURE;
            };
            let master = match std::env::var("WK_RELEASE_MASTER_PRIVATE") {
                Ok(v) if !v.trim().is_empty() => v,
                _ => {
                    eprintln!("error: WK_RELEASE_MASTER_PRIVATE must be set in the environment");
                    return ExitCode::FAILURE;
                }
            };
            match issue_release_credential(master.trim(), version) {
                Ok(cred) => {
                    eprintln!("# Per-version release credential for {version}.");
                    eprintln!("# Export these into the build env so they bake in via option_env!.");
                    println!("WK_RELEASE_PRIV_V={}", cred.private_key_hex);
                    println!("WK_RELEASE_PUB_V={}", cred.public_key_hex);
                    println!("WK_RELEASE_CERT={}", cred.cert_hex);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: release-keys <master|issue <version>>");
            ExitCode::FAILURE
        }
    }
}
