//! `Client` — reqwest-backed bearer-auth HTTP against `platform.wavekat.com`.
//!
//! Ported from `wavekat-cli/src/client.rs`. Two intentional changes vs.
//! the CLI:
//!
//!   1. Storage-agnostic constructor: `Client::new(base_url, token)`
//!      instead of `Client::from_config()`. Reading auth.json belongs in
//!      the consumer (see this crate's `CLAUDE.md`).
//!   2. Typed errors via [`crate::Error`] instead of `anyhow::Result`.
//!      Consumers that prefer `anyhow` can `?` straight through.
//!
//! Surface stays close to the CLI so the CLI's eventual migration is
//! mechanical.

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::token::Token;

/// HTTP client with the bearer token baked into its default headers.
///
/// Cheap to clone (it's a thin wrapper around `reqwest::Client`, which is
/// itself an `Arc` internally), so prefer cloning over re-building.
#[derive(Clone)]
pub struct Client {
    inner: reqwest::Client,
    base_url: String,
}

impl Client {
    /// Build a client for the given platform base URL, authenticated with
    /// `token`. The base URL's trailing slash (if any) is stripped.
    pub fn new(base_url: impl Into<String>, token: Token) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", token.as_str());
        let header = HeaderValue::from_str(&value)
            .map_err(|_| Error::BadRequest("token contained invalid bytes".into()))?;
        headers.insert(AUTHORIZATION, header);

        let inner = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!(
                "wavekat-platform-client/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()?;
        Ok(Self {
            inner,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    /// Base URL the client was configured with, with any trailing slash
    /// stripped. Useful for callers that want to print a clickable link
    /// alongside an API result (`{base_url}/projects/…`).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// `GET {path}` and decode the JSON response.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let resp = self.inner.get(&url).send().await?;
        decode(url, resp).await
    }

    /// `GET {path}?query` and decode the JSON response. `query` is any
    /// `serde::Serialize` — typically a `&[(K, V)]` or a struct.
    pub async fn get_json_query<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T> {
        let url = self.url(path);
        let resp = self.inner.get(&url).query(query).send().await?;
        decode(url, resp).await
    }

    /// `POST {path}` with `body` serialized as JSON, decode the JSON
    /// response.
    pub async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.url(path);
        let resp = self.inner.post(&url).json(body).send().await?;
        decode(url, resp).await
    }

    /// `POST {path}` with no body, expecting an empty/ignored response.
    pub async fn post_empty(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        let resp = self.inner.post(&url).send().await?;
        ensure_success(url, resp).await
    }

    /// `POST {path}` with no body, decoding the JSON response. The CLI
    /// uses this for `…/finalize` endpoints that take no body but return
    /// the updated row.
    pub async fn post_empty_returning_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.url(path);
        let resp = self.inner.post(&url).send().await?;
        decode(url, resp).await
    }

    /// `DELETE {path}`.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = self.url(path);
        let resp = self.inner.delete(&url).send().await?;
        ensure_success(url, resp).await
    }

    /// `PUT {path}` with `body` as `application/octet-stream`. Used by
    /// the CLI's `models push` to ship bytes through the platform's
    /// proxy upload route when R2 isn't directly reachable.
    pub async fn put_proxy_bytes(&self, path: &str, body: Vec<u8>) -> Result<()> {
        self.put_raw_bytes(path, "application/octet-stream", body)
            .await
    }

    /// `PUT {path}` with `body` and a caller-chosen content type. The
    /// bearer's auth header rides along (per the default-headers map),
    /// so this is for routes on the platform itself — not for
    /// presigned R2 PUTs. Voice recording bytes go through here.
    pub async fn put_raw_bytes(&self, path: &str, content_type: &str, body: Vec<u8>) -> Result<()> {
        let url = self.url(path);
        let resp = self
            .inner
            .put(&url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await?;
        ensure_success(url, resp).await
    }

    /// `PUT` raw bytes to a presigned URL. Deliberately uses a *fresh*
    /// `reqwest::Client` (no auth headers) — adding `Authorization:
    /// Bearer …` would make S3/R2 reject the request because it's not
    /// part of the SigV4 query-string signature.
    pub async fn put_presigned_bytes(presigned_url: &str, body: Vec<u8>) -> Result<()> {
        let resp = reqwest::Client::new()
            .put(presigned_url)
            .body(body)
            .send()
            .await?;
        ensure_success(presigned_url.to_string(), resp).await
    }

    /// `POST {base_url}{path}` with `body` as JSON against a public,
    /// unauthenticated platform endpoint. Deliberately builds a *fresh*
    /// `reqwest::Client` (like [`Client::put_presigned_bytes`]) so the
    /// request carries no `Authorization` header: sending a bearer to a
    /// route that doesn't expect one can trip surprising server-side
    /// branches, and a token-less request is the honest shape for an
    /// endpoint that runs before any sign-in.
    ///
    /// Used by callers that report something before a user has
    /// authenticated — e.g. the anonymous first-run install heartbeat
    /// (see [`Client::install_heartbeat`]). For authenticated writes use
    /// [`Client::post_json`].
    pub async fn post_public_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        base_url: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let base = base_url.trim_end_matches('/');
        let url = format!("{}{}", base, path);
        let resp = reqwest::Client::new().post(&url).json(body).send().await?;
        decode(url, resp).await
    }

    /// Stream a `GET` response body into `sink`. Returns the number of
    /// bytes written. Used for big payloads (manifests, audio clips)
    /// where holding the whole body in memory would be wasteful.
    pub async fn get_stream_to<W: AsyncWriteExt + Unpin>(
        &self,
        path: &str,
        sink: &mut W,
    ) -> Result<u64> {
        let url = self.url(path);
        let resp = self.inner.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(http_error(status.as_u16(), url, body));
        }
        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            sink.write_all(&bytes).await?;
            written += bytes.len() as u64;
        }
        sink.flush().await?;
        Ok(written)
    }
}

async fn decode<T: DeserializeOwned>(url: String, resp: reqwest::Response) -> Result<T> {
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), url, text));
    }
    serde_json::from_str(&text).map_err(|source| Error::Decode { url, source })
}

async fn ensure_success(url: String, resp: reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    Err(http_error(status.as_u16(), url, body))
}

/// Map an HTTP error response to the matching [`Error`] variant. 401
/// gets its own [`Error::Unauthorized`] so consumers can render a
/// tailored "sign in again" message; everything else stays as
/// [`Error::Http`].
fn http_error(status: u16, url: String, body: String) -> Error {
    let body = truncate(&body, 500).to_string();
    if status == 401 {
        Error::Unauthorized { url, body }
    } else {
        Error::Http { status, url, body }
    }
}

fn truncate(s: &str, n: usize) -> &str {
    if s.len() > n {
        // Walk back to the previous char boundary so we don't slice a
        // multibyte UTF-8 sequence (the CLI's version of this used a
        // raw byte slice, which is a panic waiting for a non-ASCII
        // error body).
        let mut end = n;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_format_matches_cli_shape() {
        // Regression guard: `Display` for `Error::Http` should format
        // "{status} {url}: {body}" — matches what the CLI's old `decode`
        // produced via `anyhow!`. Consumers (and grep-driven debugging)
        // depend on the shape.
        let e = Error::Http {
            status: 500,
            url: "https://platform.wavekat.com/api/me".into(),
            body: "boom".into(),
        };
        let s = e.to_string();
        assert!(s.contains("500"), "{s}");
        assert!(s.contains("https://platform.wavekat.com/api/me"), "{s}");
        assert!(s.contains("boom"), "{s}");
    }

    #[test]
    fn http_error_splits_401_into_unauthorized() {
        // 401 routes to the dedicated variant so consumers can match on
        // it instead of inspecting `status == 401`.
        let e = http_error(
            401,
            "https://platform.wavekat.com/api/me".into(),
            "{\"error\":\"unauthenticated\"}".into(),
        );
        assert!(
            matches!(e, Error::Unauthorized { .. }),
            "expected Unauthorized, got {e:?}"
        );
        // Display still mentions 401 + url so logs stay greppable.
        let s = e.to_string();
        assert!(s.contains("401"), "{s}");
        assert!(s.contains("https://platform.wavekat.com/api/me"), "{s}");
    }

    #[test]
    fn http_error_keeps_non_401_in_http_variant() {
        let e = http_error(
            500,
            "https://platform.wavekat.com/api/me".into(),
            "boom".into(),
        );
        assert!(
            matches!(e, Error::Http { status: 500, .. }),
            "expected Http {{ status: 500 }}, got {e:?}"
        );
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // Multi-byte char straddling the cap shouldn't panic.
        let s = "a".repeat(498) + "é"; // 'é' is 2 bytes in UTF-8.
        let t = truncate(&s, 499);
        assert!(s.starts_with(t));
    }
}
