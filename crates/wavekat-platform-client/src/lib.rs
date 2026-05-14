//! Rust client for the WaveKat platform.
//!
//! Reusable across consumers (the [`wavekat-cli`] binary `wk`, the
//! [`wavekat-voice`] desktop daemon, future WaveKat tools) so platform
//! auth and artifact upload have one implementation, not many.
//!
//! Status: early scaffolding. The first slice — `Client` (reqwest-backed
//! bearer HTTP) and the loopback OAuth handshake — is being ported from
//! [`wavekat-cli`]. See the design notes in the [`wavekat-voice`] repo,
//! `docs/13-platform-login-and-client.md`.
//!
//! [`wavekat-cli`]: https://github.com/wavekat/wavekat-cli
//! [`wavekat-voice`]: https://github.com/wavekat/wavekat-voice
