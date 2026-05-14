# 01 — Initial port: scaffold → v0.0.1

> Status: planning · Date: 2026-05-14
>
> This is the work plan to get the crate from "empty scaffold" to "usable v0.0.1 published on crates.io," at which point the first downstream consumer ([`wavekat-voice`](https://github.com/wavekat/wavekat-voice)) can depend on it. The companion design rationale — *why* this crate exists separately from `wavekat-cli` — lives in `wavekat-voice/docs/13-platform-login-and-client.md`.

## Where we are

`main` ships scaffold only:

- Workspace layout (`crates/wavekat-platform-client/`) cribbed from `wavekat-core`.
- `lib.rs` contains only an intent docstring. No public types, no public functions.
- `Cargo.toml` declares zero dependencies.
- CI + release-plz workflows pre-wired; `cargo check --workspace` is clean.

That means `cargo add wavekat-platform-client` from a consumer would compile, but `use wavekat_platform_client::Client` wouldn't — there is no `Client`. The next step is to actually port the code from [`wavekat-cli`](https://github.com/wavekat/wavekat-cli), which has a battle-tested implementation of everything v0.0.1 needs.

## What we're porting (not redesigning)

The CLI already does platform auth correctly, and its `client.rs` + `login.rs` were written with the assumption they might one day be shared. We are **moving** that code, with minimal adjustments — not redesigning it.

| Source in `wavekat-cli` | Target here | Notes |
|---|---|---|
| `src/client.rs::Client` and helpers | `crates/wavekat-platform-client/src/client.rs` | Reqwest-backed bearer-auth HTTP. The CLI-specific helpers `get_stream_to`, `put_proxy_bytes`, `put_presigned_bytes` come along — they're agnostic of the consumer. |
| `src/client.rs::decode` + `truncate` | Private inside `client.rs` | Internal. |
| `src/commands/login.rs::browser_handshake` + `handle_callback` + `respond` | `crates/wavekat-platform-client/src/oauth.rs` | The whole loopback dance, refactored as a single async-or-blocking entry point. |
| `src/commands/login.rs::random_state` + `base64url` + `html_escape` + `hostname` + `client_name` | Inside `oauth.rs` (private) | The unit tests for these (`base64url_rfc_vectors`, `base64url_uses_url_safe_alphabet`, `random_state_shape`, `random_state_is_not_constant`, `html_escape_handles_metacharacters`) port over verbatim. |
| The `Me` deserialize struct in `src/commands/me.rs` | `crates/wavekat-platform-client/src/me.rs` | Public — `Me { id, login, name, email, role }`. |
| `Client::post_empty("/api/auth/cli/tokens/revoke-current")` | `Client::revoke_current_token()` | Typed method, same endpoint. |

What is **not** ported: `config.rs` (file-backed storage), all `commands/*` other than `login`/`logout`/`me`. Storage and CLI-shaped concerns stay in the CLI per this repo's `CLAUDE.md`.

## Public surface for v0.0.1

```rust
// lib.rs — re-exports only
pub use client::Client;
pub use error::{Error, Result};
pub use me::Me;
pub use oauth::{loopback_handshake, HandshakeOptions, HandshakeOutcome};
pub use token::Token;

// client.rs
pub struct Client { /* … */ }
impl Client {
    pub fn new(base_url: impl Into<String>, token: Token) -> Result<Self>;
    pub fn base_url(&self) -> &str;

    pub async fn whoami(&self) -> Result<Me>;
    pub async fn revoke_current_token(&self) -> Result<()>;

    // Lower-level helpers — typed wrappers come on top of these.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T>;
    pub async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self, path: &str, body: &B,
    ) -> Result<T>;
    pub async fn post_empty(&self, path: &str) -> Result<()>;
    pub async fn delete(&self, path: &str) -> Result<()>;
}

// oauth.rs
pub struct HandshakeOptions {
    /// Display name shown in the user's platform "Active sessions" list.
    /// Defaults to `"wavekat-platform-client on <hostname>"`.
    pub client_name: Option<String>,
    /// How long to wait for the browser callback. Default 5 min.
    pub timeout: Duration,
}

pub struct HandshakeOutcome {
    /// The signed-in token. Hand to `Client::new`.
    pub token: Token,
    /// Echoed back from the platform — typically the user's login.
    /// Useful so callers can avoid an extra `/api/me` round-trip after sign-in.
    pub login: Option<String>,
}

/// Two-phase API so callers can show the URL in their UI before
/// blocking on the browser callback.
pub struct PendingHandshake { /* … */ }
impl PendingHandshake {
    pub fn url(&self) -> &str;
    pub fn state(&self) -> &str;
    pub async fn wait(self) -> Result<HandshakeOutcome>;
}

pub fn loopback_handshake(
    base_url: &str,
    options: HandshakeOptions,
) -> Result<PendingHandshake>;

// token.rs — newtype wrapper with redacted Debug
pub struct Token(/* private */ String);
impl Token {
    pub fn new(raw: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
}
impl fmt::Debug for Token { /* prints "Token(wkcli_***redacted)" */ }
// no Display — must opt in via .as_str()

// error.rs
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP {status} {url}: {body}")]
    Http { status: u16, url: String, body: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("decoding response from {url}: {source}")]
    Decode { url: String, source: serde_json::Error },
    #[error("OAuth state mismatch — got {actual:?}, expected {expected:?}")]
    StateMismatch { actual: Option<String>, expected: String },
    #[error("OAuth flow cancelled in browser: {0}")]
    Cancelled(String),
    #[error("OAuth handshake timed out after {0:?}")]
    Timeout(Duration),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
```

## One design adjustment vs the CLI: the crate doesn't open browsers

The CLI calls `webbrowser::open(&auth_url)` inside its handshake. That's fine for `wk login` (one process, one terminal, one consenting user). But the second consumer — `wavekat-voice` — runs as a desktop app with an Electron host and opens external URLs via `shell.openExternal`, not via the `webbrowser` crate.

So the crate **returns the URL**; the caller opens it. Concretely the two-phase API above lets a caller do:

```rust
let pending = loopback_handshake("https://platform.wavekat.com", opts)?;
// caller is free to:
//   - println!("Open: {}", pending.url())    // CLI
//   - shell.openExternal(pending.url())      // Electron / Voice
//   - webbrowser::open(pending.url())        // CLI convenience (still allowed)
let outcome = pending.wait().await?;
```

This also means the crate doesn't depend on the `webbrowser` crate — one less transitive dep on the desktop side.

## Dependency set for v0.0.1

Each justified:

| Crate | Why | Features |
|---|---|---|
| `reqwest` | HTTP client (carries over from CLI). | `rustls-tls`, `json`, `gzip`, `stream` |
| `serde` | All payload types. | `derive` |
| `serde_json` | Bodies + error-body parsing. | – |
| `url` | URL-encoding the OAuth callback params. | – |
| `rand` | Random state for CSRF (carries over from CLI). | – |
| `thiserror` | Typed `Error` enum. | – |
| `tokio` | Time + sync primitives for the `PendingHandshake::wait` future. | `time`, `sync` (NOT `rt-multi-thread` — let consumers pick) |

Total fresh adds vs the CLI's already-vetted set: just `thiserror`. Everything else is already in the CLI's tree.

**Deliberately NOT pulled:** `clap`, `webbrowser`, `dirs`, `tokio` macros / rt-multi-thread, `arrow*`, `parquet`, `hound`, `rayon`, `anyhow`. Each one would shape this crate's character in the wrong direction.

## File layout

```
crates/wavekat-platform-client/src/
├── lib.rs        # crate docstring + pub use re-exports only
├── client.rs     # Client + low-level get/post/delete helpers
├── error.rs      # Error enum (thiserror) + Result alias
├── me.rs         # Me struct + Client::whoami()
├── oauth.rs      # loopback_handshake, PendingHandshake, HandshakeOptions/Outcome
└── token.rs      # Token newtype with redacted Debug
```

Five small files. Each module's preamble carries the WHY-comment style the CLI established (see `commands/login.rs:1-22` for the canonical example).

## Test plan

### In-repo (CI-runnable)

- **Unit tests carried over verbatim from `wavekat-cli`:**
  - `base64url_rfc_vectors`
  - `base64url_uses_url_safe_alphabet`
  - `random_state_shape`
  - `random_state_is_not_constant`
  - `html_escape_handles_metacharacters`
- **New unit tests:**
  - `Token::Debug` redacts the secret (`assert!(format!("{:?}", t).contains("***"))` and `!contains(secret)`).
  - `HandshakeOptions::default()` round-trips a sensible `client_name`.
  - `Error::Http` formats the way the CLI's `decode` used to (so we don't regress error UX).

### Manual smoke (not in CI)

Done by a developer with platform credentials, against a running `platform.wavekat.com` (or staging):

1. `cargo run --example smoke -- login` — completes the loopback dance, prints the new `Token` (redacted) and the `Me` row. (Add an `examples/smoke.rs` for this; uses `webbrowser::open` for convenience since it's a binary.)
2. Re-run `cargo run --example smoke -- whoami --token $TOKEN` — succeeds without re-prompting.
3. `cargo run --example smoke -- revoke --token $TOKEN` — succeeds; subsequent whoami with the same token returns 401.

Don't put platform credentials in CI for v0.0.1; the cost-to-reward isn't worth it yet. Manual smoke covers the surface that mocks can't.

## Publishing v0.0.1

Gate the publish on:

- All in-repo tests green on CI (`make ci` locally; `ci.yml` on GitHub).
- Manual smoke passed once against the live platform.
- `cargo publish --dry-run -p wavekat-platform-client` clean.
- README's [Status](../README.md#status) table updated to show v0.0.1 surfaces as ✅ instead of "Coming in v0.0.1."

Mechanically: merge to `main` with a Conventional Commit subject (`feat: port Client and loopback OAuth from wavekat-cli`). [`release-plz`](../release-plz.toml) picks it up, opens a release PR, and on merge of the release PR cuts the tag and publishes to crates.io.

## What's explicitly out of v0.0.1

- **Artifact upload** (3-step create → presigned PUT → finalize). The CLI's `commands/models.rs` has the pattern, but `wavekat-voice` doesn't have a recording to upload yet. Lands in v0.0.2 when Voice's recording PR is ready to consume it.
- **CLI migration.** A follow-up PR on the `wavekat-cli` repo will rewrite its `client.rs` + the relevant half of `login.rs` to depend on this crate. Sequenced after v0.0.1 ships; the CLI keeps working unchanged in the meantime.
- **Local (file-backed) storage helper.** Even though the CLI uses it today, baking a storage policy into this crate would undo the storage-agnostic principle (CLAUDE.md). The CLI keeps its own `config.rs`; if a third consumer ever wants the same file-backed flow we can extract a `wavekat-platform-client-fsstore` companion.
- **OAuth refresh / device-code / PKCE.** Loopback is what the platform supports today; if/when the platform adds other flows we'll add them here. Not blocking v0.0.1.

## Open design questions

Defaults are baked into the surface sketch above; flag dissent before implementation lands.

1. **`thiserror` vs `anyhow` for the public Error.** CLI uses `anyhow` because it's an end-user binary; libraries traditionally pick `thiserror` so consumers can match on variants. Default: **`thiserror`** as sketched above. The CLI itself, once it migrates, can `?` these errors into its own `anyhow::Result` painlessly.
2. **Token as a newtype.** Adding `Token(String)` with redacted `Debug` costs nothing and avoids ever accidentally logging the secret. Default: **yes**, ship as `Token` from v0.0.1. Consumer ergonomics: `Token::new(s)` and `t.as_str()`. No `Display`, no `From<String>` impl that allows accidental logging via `format!("{}", t)`.
3. **Sync handshake, async client.** The CLI's `browser_handshake` is sync (`std::net::TcpListener`, blocking `accept`). The rest of the CLI is async. Library proposal: keep that mix — sync `loopback_handshake` (returns `PendingHandshake`, a struct holding the listener), async `PendingHandshake::wait` that internally `spawn_blocking`s the accept loop. Avoids dragging in `tokio::net` and the runtime opinions that come with it, while still presenting an `.await`-able tail. Default: **as sketched.**
4. **Hostname source.** CLI calls `hostname` via `std::process::Command` on Unix and `COMPUTERNAME` env on Windows. Works. Alternative: `gethostname` crate (~50 LOC, no fork). Default: **port the CLI's approach verbatim**; revisit if the spawn becomes a real cost.
5. **MSRV.** Set `rust-version = "1.75"` (or whatever matches `wavekat-cli`'s and `wavekat-core`'s implicit MSRV). Pin before publish so consumers know.

## Sequencing

After this doc merges:

1. **PR `feat/initial-client-port`** on this repo. Lands all five `src/*.rs` files + the unit tests + an `examples/smoke.rs`. Conventional title: `feat: port Client and loopback OAuth from wavekat-cli`. Single PR; the modules are small.
2. **Manual smoke** against `platform.wavekat.com` per [Test plan](#manual-smoke-not-in-ci).
3. **Release-plz cuts v0.0.1.** Auto-published on merge of the release PR.
4. **`wavekat-voice` consumes it.** PR 2 in [`wavekat-voice/docs/13-platform-login-and-client.md`](https://github.com/wavekat/wavekat-voice/blob/main/docs/13-platform-login-and-client.md) — daemon-side sign-in, keychain storage, Platform settings page.
5. **`wavekat-cli` migrates.** Standalone follow-up PR on the CLI repo; replaces its `client.rs` and the handshake half of `login.rs` with calls into this crate. Validates that the surface really does fit both consumers.

## What this doc is not

- Not a v0.0.2+ plan. Artifact upload, the upload queue, recording-disclosure plumbing — all later.
- Not a deviation from the CLI's design. Where defaults change (no browser-open in the crate, `Token` newtype, `thiserror` typed errors), they're called out above with reasoning.
- Not a hard commitment to the surface above. Five open questions; defaults stand unless redirected before PR 1 lands.
