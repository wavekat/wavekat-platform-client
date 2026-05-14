//! Loopback OAuth handshake.
//!
//! Mirrors the flow that `wavekat-cli`'s `wk login` runs today:
//!
//!   1. Bind a TCP listener on `127.0.0.1:<random ephemeral port>`.
//!   2. Generate a one-shot CSRF state.
//!   3. Compute the platform's `/cli-login` URL with
//!      `?callback=…&state=…&client=…[&source=…]` and hand it back to
//!      the caller via [`PendingHandshake::url`].
//!   4. Caller opens the URL however they want — `webbrowser::open` for a
//!      CLI, `shell.openExternal` for an Electron app, plain `println!`
//!      for a remote host with no browser.
//!   5. Caller awaits [`PendingHandshake::wait`], which blocks (on a
//!      worker thread) until the platform redirects the browser back to
//!      the loopback URL with `?token=…&state=…` (success) or
//!      `?error=…&state=…` (cancel).
//!
//! The crate intentionally does **not** call `webbrowser::open` itself —
//! that decision is the consumer's. See `docs/01-initial-port.md` for why.
//!
//! The loopback HTTP server is hand-rolled (`std::net`) for the same
//! reason as the CLI: one request, one response, no need to drag in a
//! framework. Reads are bounded so a stray local probe can't tie us up.

use rand::RngCore;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::token::Token;

/// Tunables for [`loopback_handshake`].
#[derive(Debug, Clone)]
pub struct HandshakeOptions {
    /// Short app identifier sent as the `client` query param. Shown as
    /// the consent screen title and in the user's "Active sessions"
    /// listing. Defaults to `"wavekat-platform-client"`; consumers
    /// usually want to override with their own product name (e.g.
    /// `"wavekat-voice"`).
    pub client: Option<String>,
    /// Origin label sent as the `source` query param. Shown beside the
    /// title as "from <source>" and in the listing. Typically the
    /// machine's hostname; defaults to that. Set to `Some("")` (or
    /// override [`Self::omit_source`] to true) to suppress it for
    /// privacy.
    pub source: Option<String>,
    /// If true, no `source` is sent at all — the platform will store
    /// `null` and the consent UI won't render a "from …" line. Useful
    /// for desktop apps that don't want to disclose the hostname.
    pub omit_source: bool,
    /// How long to wait for the browser callback. Default: 5 min.
    pub timeout: Duration,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            client: None,
            source: None,
            omit_source: false,
            timeout: Duration::from_secs(5 * 60),
        }
    }
}

/// Result of a successful handshake.
#[derive(Debug)]
pub struct HandshakeOutcome {
    /// The signed-in token. Hand to [`crate::Client::new`].
    pub token: Token,
    /// Echoed back from the platform — typically the user's login.
    /// Useful so callers can skip an extra `/api/me` round-trip right
    /// after sign-in. `None` when the platform didn't include it (older
    /// platforms, or the `error=` path).
    pub login: Option<String>,
}

/// In-flight handshake. Returned by [`loopback_handshake`] before the
/// caller decides how to surface the URL.
///
/// Drop this without calling [`PendingHandshake::wait`] to abandon the
/// flow; the listener closes when the value goes out of scope.
pub struct PendingHandshake {
    listener: TcpListener,
    url: String,
    state: String,
    timeout: Duration,
}

impl PendingHandshake {
    /// The URL to open in the user's browser. Hand this to whatever
    /// "open external URL" facility the consumer has — `webbrowser::open`,
    /// `shell.openExternal`, or `println!`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The CSRF state we generated. Mostly useful for tests / debugging;
    /// callers don't normally need to look at it.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Block (on a worker thread) until the browser redirects back, or
    /// the timeout fires.
    pub async fn wait(self) -> Result<HandshakeOutcome> {
        let PendingHandshake {
            listener,
            state,
            timeout,
            ..
        } = self;

        // The accept loop is sync-by-nature (`std::net::TcpListener`),
        // so we run it on a blocking worker. The deadline lives inside
        // the worker so we never strand a thread on a permanently-open
        // listener — set_nonblocking + sleep poll until the deadline.
        let join = tokio::task::spawn_blocking(move || accept_loop(listener, &state, timeout));
        match join.await {
            Ok(result) => result,
            Err(e) => Err(Error::BadRequest(format!(
                "loopback worker panicked or was cancelled: {e}"
            ))),
        }
    }
}

/// Bind the loopback listener and compute the platform sign-in URL.
///
/// This is sync because binding a `std::net::TcpListener` is sync — there's
/// no `.await` to be had. The async work (waiting for the browser
/// redirect) lives on [`PendingHandshake::wait`].
pub fn loopback_handshake(base_url: &str, options: HandshakeOptions) -> Result<PendingHandshake> {
    // Loopback only — never bind 0.0.0.0. Anything bound to a non-loopback
    // interface could be reached by another host on the LAN for the brief
    // window we listen, and the token in the redirect URL is a credential.
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))?;
    let port = listener.local_addr()?.port();

    let state = random_state();
    let client = options
        .client
        .unwrap_or_else(|| "wavekat-platform-client".to_string());
    let callback = format!("http://127.0.0.1:{port}/callback");

    let base = base_url.trim_end_matches('/');
    let mut url = format!(
        "{base}/cli-login?callback={cb}&state={st}&client={cl}",
        cb = url_encode(&callback),
        st = url_encode(&state),
        cl = url_encode(&client),
    );
    if !options.omit_source {
        let source = options.source.unwrap_or_else(default_source);
        if !source.is_empty() {
            url.push_str("&source=");
            url.push_str(&url_encode(&source));
        }
    }

    Ok(PendingHandshake {
        listener,
        url,
        state,
        timeout: options.timeout,
    })
}

fn accept_loop(
    listener: TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<HandshakeOutcome> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;

    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout(timeout));
        }
        match listener.accept() {
            Ok((stream, _addr)) => {
                // Switch the accepted stream back to blocking for the
                // tiny request/response we're about to do.
                stream.set_nonblocking(false)?;
                match handle_callback(stream, expected_state) {
                    Ok(HandlerResult::Got(outcome)) => return Ok(outcome),
                    Ok(HandlerResult::KeepListening) => continue,
                    Err(e @ Error::StateMismatch { .. }) => return Err(e),
                    Err(e @ Error::Cancelled(_)) => return Err(e),
                    Err(_) => {
                        // A stray probe (favicon, devtools HEAD, etc.)
                        // shouldn't break the real one. Keep listening.
                        continue;
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

enum HandlerResult {
    Got(HandshakeOutcome),
    KeepListening,
}

fn handle_callback(mut stream: TcpStream, expected_state: &str) -> Result<HandlerResult> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    // Read just the request line + headers. Cap at 8 KiB — the URL we
    // care about is well under that, and we never need the body.
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut header_bytes = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        header_bytes += n;
        if header_bytes > 8192 {
            return Err(Error::BadRequest("request headers too large".into()));
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method != "GET" {
        respond(&mut stream, 405, "method not allowed", "method not allowed")?;
        return Ok(HandlerResult::KeepListening);
    }
    if !target.starts_with("/callback") {
        // Browsers fetch /favicon.ico; some OSes probe /. Reply 404 and
        // keep listening — only /callback matters.
        respond(&mut stream, 404, "not found", "not found")?;
        return Ok(HandlerResult::KeepListening);
    }

    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut token: Option<String> = None;
    let mut state: Option<String> = None;
    let mut error: Option<String> = None;
    let mut login: Option<String> = None;
    for (k, v) in parse_query(query) {
        match k.as_str() {
            "token" => token = Some(v),
            "state" => state = Some(v),
            "error" => error = Some(v),
            "login" => login = Some(v),
            _ => {}
        }
    }

    if state.as_deref() != Some(expected_state) {
        respond(
            &mut stream,
            400,
            "bad state",
            "<h1>State mismatch</h1><p>Re-run the sign-in to start over.</p>",
        )?;
        return Err(Error::StateMismatch {
            actual: state,
            expected: expected_state.to_string(),
        });
    }

    if let Some(err) = error {
        respond(
            &mut stream,
            200,
            "OK",
            &format!(
                "<h1>Login cancelled</h1><p>You can close this tab and try again.</p><p style='color:#888'>reason: {}</p>",
                html_escape(&err),
            ),
        )?;
        return Err(Error::Cancelled(err));
    }

    let Some(tok) = token else {
        respond(&mut stream, 400, "missing token", "missing token")?;
        return Err(Error::BadRequest("callback missing token".into()));
    };

    respond(
        &mut stream,
        200,
        "OK",
        "<!doctype html><html><head><meta charset=utf-8><title>WaveKat sign-in complete</title><style>body{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;color:#1a1a1a}</style></head><body><h1>You're signed in.</h1><p>You can close this tab and return to the app.</p></body></html>",
    )?;
    Ok(HandlerResult::Got(HandshakeOutcome {
        token: Token::new(tok),
        login,
    }))
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> Result<()> {
    let body_bytes = body.as_bytes();
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body_bytes.len(),
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body_bytes)?;
    let _ = stream.flush();
    // Drain a tiny amount of any unread bytes so the kernel doesn't RST
    // the connection before the browser reads our response.
    let mut sink = [0u8; 64];
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.read(&mut sink);
    Ok(())
}

fn random_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64url(&bytes)
}

// URL-safe base64 (no padding). Hand-rolled to avoid pulling in another
// crate just for 24 bytes of CSRF state.
fn base64url(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHA[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char);
    }
    out
}

fn default_source() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| hostname().ok())
        .unwrap_or_else(|| "unknown-host".to_string())
}

#[cfg(unix)]
fn hostname() -> Result<String> {
    let out = std::process::Command::new("hostname").output()?;
    if !out.status.success() {
        return Err(Error::BadRequest("hostname exited non-zero".into()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(not(unix))]
fn hostname() -> Result<String> {
    std::env::var("COMPUTERNAME")
        .map_err(|e| Error::BadRequest(format!("COMPUTERNAME not set: {e}")))
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(q.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Carried verbatim from `wavekat-cli/src/commands/login.rs`.

    #[test]
    fn base64url_rfc_vectors() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64url_uses_url_safe_alphabet() {
        assert_eq!(base64url(&[0xfb, 0xff, 0xff]), "-___");
        let big: Vec<u8> = (0u8..=255).collect();
        let out = base64url(&big);
        assert!(!out.contains('+'));
        assert!(!out.contains('/'));
        assert!(!out.contains('='));
    }

    #[test]
    fn random_state_shape() {
        let s = random_state();
        assert_eq!(s.len(), 32);
        let alpha: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        for b in s.as_bytes() {
            assert!(alpha.contains(b), "unexpected byte {b:#x} in state");
        }
    }

    #[test]
    fn random_state_is_not_constant() {
        assert_ne!(random_state(), random_state());
    }

    #[test]
    fn html_escape_handles_metacharacters() {
        assert_eq!(
            html_escape("<a href=\"x\">it's & ok</a>"),
            "&lt;a href=&quot;x&quot;&gt;it&#39;s &amp; ok&lt;/a&gt;",
        );
        assert_eq!(html_escape("plain text"), "plain text");
        assert_eq!(html_escape(""), "");
    }

    // New tests for the v0.0.1 surface.

    #[test]
    fn handshake_options_default_has_sensible_timeout() {
        let opts = HandshakeOptions::default();
        assert_eq!(opts.timeout, Duration::from_secs(300));
        assert!(opts.client.is_none());
        assert!(opts.source.is_none());
        assert!(!opts.omit_source);
    }

    #[test]
    fn default_source_falls_back_to_a_hostname() {
        let s = default_source();
        assert!(!s.is_empty(), "should never produce an empty source");
    }

    #[test]
    fn loopback_handshake_returns_url_with_loopback_callback() {
        let pending =
            loopback_handshake("https://platform.wavekat.com", HandshakeOptions::default())
                .expect("bind loopback");
        let url = pending.url();
        assert!(url.starts_with("https://platform.wavekat.com/cli-login?"));
        assert!(url.contains("127.0.0.1"), "{url}");
        assert!(url.contains(&format!(
            "state={}",
            url::form_urlencoded::byte_serialize(pending.state().as_bytes()).collect::<String>()
        )));
        // Default `client` is the crate name.
        assert!(
            url.contains("client=wavekat-platform-client"),
            "expected client=wavekat-platform-client in {url}",
        );
        // Default options send a source (the hostname).
        assert!(url.contains("&source="), "expected &source=... in {url}");
    }

    #[test]
    fn loopback_handshake_uses_explicit_client_and_source() {
        let pending = loopback_handshake(
            "https://platform.wavekat.com",
            HandshakeOptions {
                client: Some("wavekat-voice".into()),
                source: Some("studio-mac".into()),
                ..Default::default()
            },
        )
        .expect("bind loopback");
        let url = pending.url();
        assert!(url.contains("client=wavekat-voice"), "{url}");
        assert!(url.contains("source=studio-mac"), "{url}");
    }

    #[test]
    fn loopback_handshake_omits_source_when_requested() {
        let pending = loopback_handshake(
            "https://platform.wavekat.com",
            HandshakeOptions {
                client: Some("wavekat-voice".into()),
                omit_source: true,
                ..Default::default()
            },
        )
        .expect("bind loopback");
        let url = pending.url();
        assert!(url.contains("client=wavekat-voice"), "{url}");
        assert!(!url.contains("source="), "should not include source: {url}");
    }

    #[test]
    fn loopback_handshake_strips_trailing_slash() {
        let pending = loopback_handshake("https://platform.wavekat.com/", Default::default())
            .expect("bind loopback");
        assert!(pending
            .url()
            .starts_with("https://platform.wavekat.com/cli-login?"));
    }
}
