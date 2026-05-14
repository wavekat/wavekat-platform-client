[![Crates.io](https://img.shields.io/crates/v/wavekat-platform-client.svg)](https://crates.io/crates/wavekat-platform-client)
[![docs.rs](https://docs.rs/wavekat-platform-client/badge.svg)](https://docs.rs/wavekat-platform-client)

Rust client for the [WaveKat platform](https://platform.wavekat.com) — auth, sessions, artifact upload.

> [!WARNING]
> Early scaffolding. No public API yet. See [Status](#status) below.

## Why this crate exists

The WaveKat product surface includes several Rust consumers that need to talk to the same platform:

- [`wavekat-cli`](https://github.com/wavekat/wavekat-cli) — `wk`, the developer CLI (login, projects, exports).
- [`wavekat-voice`](https://github.com/wavekat/wavekat-voice) — the desktop daemon, which will upload call recordings and transcripts to the platform on the user's behalf.
- Future WaveKat tools.

Today the auth + HTTP plumbing lives inside `wavekat-cli`'s binary. Pulling a CLI tool into a Rust app to reuse its auth code is the wrong shape: it drags in CLI-only dependencies (`clap`, `arrow`, `parquet`, …), the name reads wrong in another app's `Cargo.toml`, and the CLI's release cadence couples to consumers that have nothing to do with `wk` features.

This crate is the one place that knows how to talk to `platform.wavekat.com`. Each consumer wires its own storage policy (the CLI uses a JSON file at `~/.config/wavekat/auth.json`; `wavekat-voice` uses the OS keychain) — the crate itself stores nothing.

## Status

| Surface | State |
|---|---|
| `Client` (reqwest-backed bearer HTTP) | Coming in v0.0.1 — port from `wavekat-cli/src/client.rs`. |
| Loopback OAuth handshake | Coming in v0.0.1 — port from `wavekat-cli/src/commands/login.rs`. |
| `/api/me` typed wrapper | v0.0.1. |
| Token revoke (`/api/auth/cli/tokens/revoke-current`) | v0.0.1. |
| Artifact upload (3-step: create → presigned PUT → finalize) | v0.0.2+ — once recording lands in `wavekat-voice`. |
| CLI migration to depend on this crate | Follow-up. The CLI keeps working as-is until then. |

## Design

- **Zero opinion on storage.** The crate exposes a `Client::new(base_url, token)` constructor. Consumers load the token from wherever fits — keychain, file, env var, in-memory test fixture — and hand it in.
- **Single bearer token shape**: `wk_…` issued by `POST /api/auth/cli/tokens`. The route path is historical (the CLI was the only consumer originally); the platform mints the same kind of token for any caller that completes the loopback OAuth flow.
- **No async runtime opinion in the surface** — uses `reqwest` async with whatever runtime the consumer brings (tokio in practice).

## License

Apache-2.0. See [`LICENSE`](LICENSE).
