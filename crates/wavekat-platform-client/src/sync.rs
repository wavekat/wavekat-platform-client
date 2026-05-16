//! Platform sync endpoints — uniform "batch upload + cursor list" shape.
//!
//! Every client→platform sync (calls today; recordings, transcripts,
//! summaries later) goes through the [`SyncEndpoint`] trait. Each
//! resource is a zero-sized marker type (e.g. [`crate::voice::VoiceCalls`])
//! that nails down:
//!
//! - the URL segment under `/api/voice/` (`RESOURCE`);
//! - the wire-shape [`SyncEndpoint::Record`] type;
//! - the typed [`SyncEndpoint::Query`] for GET pagination.
//!
//! [`Client::sync`] and [`Client::list`] are the only two methods you
//! need on the consumer side — both are parameterised by the marker.
//!
//! See `wavekat-voice/docs/21-platform-call-history-sync.md` for the
//! full design rationale and the wire contract.

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::client::Client;
use crate::error::Result;

/// One sync-able platform resource.
///
/// Implemented by zero-sized marker types — you call methods like
/// `client.sync::<VoiceCalls>(&items)` rather than constructing a
/// `VoiceCalls` value.
pub trait SyncEndpoint {
    /// Path segment under `/api/voice/`. e.g. `"calls"`, `"recordings"`.
    ///
    /// Combined into the full paths
    /// `POST /api/voice/{RESOURCE}/sync` and
    /// `GET  /api/voice/{RESOURCE}`.
    const RESOURCE: &'static str;

    /// One row's worth of data. Must round-trip through JSON; the wire
    /// shape uses camelCase per the platform's Hono/Zod convention
    /// (apply `#[serde(rename_all = "camelCase")]` on your struct).
    type Record: Serialize + DeserializeOwned + Send + Sync + 'static;

    /// Query params for `GET /api/voice/{RESOURCE}`. Typically a cursor
    /// (`before` as RFC 3339) plus a `limit` and any resource-specific
    /// filters (e.g. `account_id`). Serialized as URL query.
    type Query: Serialize + Send + Sync;
}

/// Body shape for `POST /api/voice/{R}/sync`.
///
/// `items` is the batch. The server caps batches at 100 — chunking
/// is the consumer's responsibility (the daemon's `Uploader<E>` does
/// this automatically; ad-hoc callers should too).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest<R> {
    pub items: Vec<R>,
}

/// Response from `POST /api/voice/{R}/sync`.
///
/// `accepted` counts rows the platform actually wrote (insert *or*
/// idempotent update). `skipped` counts rows the platform deliberately
/// ignored — reserved for future mutable resources where a stale
/// revision should be dropped without erroring. Always 0 for the
/// immutable calls/recordings/transcripts shipped today; consumers
/// can ignore it for now and still be forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    pub accepted: u32,
    pub skipped: u32,
}

/// One page of `GET /api/voice/{R}`.
///
/// `items` is newest-first. `next_before` is the cursor for the next
/// page (pass it back as the request's `before` field); absent/None
/// means the caller has reached the start of history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<R> {
    pub items: Vec<R>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before: Option<String>,
}

impl Client {
    /// `POST /api/voice/{E::RESOURCE}/sync` — idempotent batch upload.
    ///
    /// The platform upserts keyed by `(user_id, item.source_id)`, so
    /// retries after a flaky connection are safe.
    ///
    /// **Batch size.** The platform rejects batches over 100 items with
    /// HTTP 413. This method does *not* chunk for you — pass a slice
    /// you're confident about, or use the daemon's `Uploader<E>` which
    /// chunks at 50.
    pub async fn sync<E: SyncEndpoint>(&self, items: &[E::Record]) -> Result<SyncResponse>
    where
        E::Record: Clone,
    {
        let path = format!("/api/voice/{}/sync", E::RESOURCE);
        let body = SyncRequest {
            items: items.to_vec(),
        };
        self.post_json::<SyncResponse, _>(&path, &body).await
    }

    /// `GET /api/voice/{E::RESOURCE}` — one page of the caller's rows,
    /// newest first, scoped server-side to the bearer's user.
    pub async fn list<E: SyncEndpoint>(&self, query: &E::Query) -> Result<Page<E::Record>> {
        let path = format!("/api/voice/{}", E::RESOURCE);
        self.get_json_query::<Page<E::Record>, _>(&path, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal marker so the trait surface is exercised independently
    // of any specific resource type.
    struct DummyResource;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct DummyRecord {
        source_id: String,
        payload: String,
    }

    #[derive(Debug, Default, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DummyQuery {
        before: Option<String>,
        limit: Option<u32>,
    }

    impl SyncEndpoint for DummyResource {
        const RESOURCE: &'static str = "dummy";
        type Record = DummyRecord;
        type Query = DummyQuery;
    }

    #[test]
    fn sync_request_serializes_with_items_field() {
        let body = SyncRequest::<DummyRecord> {
            items: vec![
                DummyRecord {
                    source_id: "a".into(),
                    payload: "x".into(),
                },
                DummyRecord {
                    source_id: "b".into(),
                    payload: "y".into(),
                },
            ],
        };
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains("\"items\":["), "missing items envelope: {s}");
        assert!(s.contains("\"sourceId\":\"a\""), "wire should be camelCase: {s}");
    }

    #[test]
    fn sync_response_parses_platform_shape() {
        let raw = r#"{"accepted": 3, "skipped": 0}"#;
        let parsed: SyncResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.accepted, 3);
        assert_eq!(parsed.skipped, 0);
    }

    #[test]
    fn page_round_trip_without_cursor() {
        // The wire either omits next_before or sends null when there's
        // no more history. Both should parse to None.
        let with_null = r#"{"items": [], "nextBefore": null}"#;
        let omitted = r#"{"items": []}"#;
        let p1: Page<DummyRecord> = serde_json::from_str(with_null).unwrap();
        let p2: Page<DummyRecord> = serde_json::from_str(omitted).unwrap();
        assert!(p1.next_before.is_none());
        assert!(p2.next_before.is_none());
    }

    #[test]
    fn resource_const_drives_path() {
        // Sanity check — the trait constant is what ends up in the URL.
        assert_eq!(<DummyResource as SyncEndpoint>::RESOURCE, "dummy");
    }
}
