# WaveKat Platform Client — Project Instructions

Rust client for the WaveKat platform. The one place where auth + HTTP plumbing against `platform.wavekat.com` lives. Consumed by `wavekat-cli`, `wavekat-voice`, and future WaveKat tools.

## Purpose

Make "talk to the WaveKat platform from Rust" a solved problem so each consumer doesn't reinvent (or worse, fork) the same `reqwest` + bearer-token + OAuth-handshake plumbing.

## What belongs here

- `Client` — reqwest-backed HTTP client with bearer auth attached.
- Loopback OAuth handshake (`platform.wavekat.com/cli-login` → loopback `127.0.0.1:<ephemeral>/callback`).
- Typed wrappers for stable platform endpoints used by multiple consumers (`/api/me`, token revoke, artifact upload).
- Error types covering network, deserialization, auth-state mismatches.

## What does NOT belong here

- **Credential storage policy.** Consumers pick: `wavekat-cli` writes a JSON file at `~/.config/wavekat/auth.json`; `wavekat-voice` uses the OS keychain via the `keyring` crate. The crate's surface takes a `token: String` and returns one — it never reads or writes disk.
- **CLI-shaped concerns.** Argument parsing, terminal rendering, progress bars, anything `clap`/`unicode-width` shaped. Those stay in `wavekat-cli`.
- **Consumer-specific endpoints.** If only one product calls it, it stays in that product. Promote to this crate when a second consumer needs it.
- **Async runtime.** Use `reqwest` async; let consumers bring tokio.

## Design principles

1. **Storage-agnostic.** `Client::new(base_url, token)` — that's the contract. Anything fancier (token refresh, multi-tenant routing) goes through traits the consumer implements.
2. **Stable surface.** Consumers depend on this; breaking changes ripple. Bump minor (or major after 1.0) deliberately.
3. **Small dep set.** `reqwest`, `serde`, `url`, `anyhow`/`thiserror`. No CLI deps. No audio deps. No bin-only deps. If you're tempted to add `clap` or `parquet`, stop.
4. **Releases via release-plz.** Conventional Commits in PR titles drive the changelog (mirror of `wavekat-voice`'s rule).

## Conventions

- Apache-2.0 licensed; matches `wavekat-cli` and `wavekat-core`.
- Workspace layout: root `Cargo.toml` is `[workspace]`, real crate lives in `crates/wavekat-platform-client/`. Lets us add focused sub-crates later (e.g. `wavekat-platform-client-mock` for downstream test fixtures) without restructuring.
- Bearer tokens use the `wk_…` prefix. (Was `wkcli_` while the CLI was the only consumer; renamed in the platform before any real users existed, see wavekat-platform PR #116.) The prefix is just a visual marker — cryptographic strength is in the entropy after it.

## Related repos

- [`wavekat-cli`](https://github.com/wavekat/wavekat-cli) — first consumer; the code in this crate is ported from there.
- [`wavekat-voice`](https://github.com/wavekat/wavekat-voice) — second consumer; design notes for the integration live in that repo's `docs/13-platform-login-and-client.md`.
- [`wavekat-platform`](https://github.com/wavekat/wavekat-platform) — the server side. The endpoints this crate calls are defined there.
