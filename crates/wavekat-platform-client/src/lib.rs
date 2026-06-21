//! Rust client for the WaveKat platform.
//!
//! Reusable across consumers (the [`wavekat-cli`] binary `wk`, the
//! WaveKat desktop daemon, future WaveKat tools) so platform auth and
//! HTTP plumbing have one implementation, not many.
//!
//! ## Quick start
//!
//! ```no_run
//! use wavekat_platform_client::{loopback_handshake, Client, HandshakeOptions};
//!
//! # async fn run() -> wavekat_platform_client::Result<()> {
//! let pending = loopback_handshake(
//!     "https://platform.wavekat.com",
//!     HandshakeOptions::default(),
//! )?;
//! println!("Open: {}", pending.url());
//! let outcome = pending.wait().await?;
//!
//! let client = Client::new("https://platform.wavekat.com", outcome.token)?;
//! let me = client.whoami().await?;
//! println!("signed in as {}", me.login);
//! # Ok(()) }
//! ```
//!
//! ## What this crate is (and isn't)
//!
//! - **Storage-agnostic.** `Client::new(base_url, token)` is the contract.
//!   The crate never reads or writes disk; consumers pick where the token
//!   lives (config file, OS keychain, env var, in-memory test fixture).
//! - **Browser-agnostic.** [`loopback_handshake`] returns the sign-in URL;
//!   the caller decides how to open it (`webbrowser::open`,
//!   `shell.openExternal`, `println!`, …).
//! - **Runtime-light.** Async surface uses `reqwest`; consumers bring
//!   their own tokio runtime.
//!
//! [`wavekat-cli`]: https://github.com/wavekat/wavekat-cli

mod client;
mod error;
mod me;
mod oauth;
mod sign;
mod sync;
mod token;
mod voice;

pub use client::Client;
pub use error::{Error, Result};
pub use me::Me;
pub use oauth::{loopback_handshake, HandshakeOptions, HandshakeOutcome, PendingHandshake};
pub use sign::{
    cert_payload, generate_keypair, generate_master, issue_release_credential, verify_cert,
    verify_request, ReleaseCredential, RequestSignature,
};
pub use sync::{HasSyncEnvelope, Page, SyncEndpoint, SyncEnvelope, SyncRequest, SyncResponse};
pub use token::Token;
pub use voice::{
    InstallHeartbeatRequest, InstallHeartbeatResponse, ShareRecordingRequest,
    ShareRecordingResponse, ShareVisibility, SystemInfo, VoiceAccountRecord, VoiceAccounts,
    VoiceAccountsQuery, VoiceCallDirection, VoiceCallDisposition, VoiceCallEndReason,
    VoiceCallRecord, VoiceCalls, VoiceCallsQuery, VoiceRecordingRecord, VoiceRecordingSyncItem,
    VoiceRecordings, VoiceRecordingsQuery, VoiceRecordingsSyncResponse, VoiceTranscriptChannel,
    VoiceTranscriptRecord, VoiceTranscripts, VoiceTranscriptsQuery, VoiceTransport,
};
