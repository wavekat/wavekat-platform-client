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
use serde_json::Value as JsonValue;

use crate::client::Client;
use crate::error::Result;

/// Wire-level envelope that every sync record carries.
///
/// Embedded in each `SyncEndpoint::Record` via `#[serde(flatten)]`
/// so the two fields end up at the top of the JSON object alongside
/// the resource-specific columns. Lets the version-skew story live
/// in one place — not duplicated on every resource type.
///
/// **`schema_version`**: which wire shape the daemon wrote this row
/// with. `None` on the wire means "I'm not declaring one; treat as
/// `1`." `Client::sync` fills this in from
/// [`SyncEndpoint::CURRENT_SCHEMA_VERSION`] when a consumer leaves
/// it [`None`].
///
/// **`extras`**: free-form JSON map for fields the consumer's
/// schema version recognises but the platform's doesn't yet. The
/// platform persists `extras` verbatim so a future deploy can
/// promote a field out of it into a typed column without data loss.
/// The platform deliberately does *not* echo `extras` back on GET —
/// it's an internal-storage construct, not a public field.
///
/// Both fields are optional in serialization so a row that ships
/// neither stays on the small/fast path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras: Option<JsonValue>,
}

impl SyncEnvelope {
    /// Build an envelope stamped with the endpoint's
    /// `CURRENT_SCHEMA_VERSION`. Useful for daemon-side code that
    /// wants to construct a record with the version already filled
    /// in — `Client::sync` will also fill it in lazily, but doing it
    /// at construction time keeps tests and logs honest.
    pub fn for_endpoint<E: SyncEndpoint + ?Sized>() -> Self {
        Self {
            schema_version: Some(E::CURRENT_SCHEMA_VERSION),
            extras: None,
        }
    }
}

/// Records that carry a [`SyncEnvelope`] expose it via this trait so
/// the bridge crate can stamp the `schemaVersion` field uniformly
/// across resources. One-line impl per record type:
///
/// ```ignore
/// impl HasSyncEnvelope for VoiceCallRecord {
///     fn envelope_mut(&mut self) -> &mut SyncEnvelope { &mut self.envelope }
/// }
/// ```
pub trait HasSyncEnvelope {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope;
}

/// Clone `items` and fill in `schemaVersion` on every record whose
/// envelope left it unset. Records that supplied an explicit version
/// are passed through unchanged — useful for tests and for the rare
/// "deliberately ship an older version during a rollback" case.
fn stamp_schema_version<E: SyncEndpoint>(items: &[E::Record]) -> Vec<E::Record>
where
    E::Record: Clone + HasSyncEnvelope,
{
    let mut out = items.to_vec();
    for item in &mut out {
        let env = item.envelope_mut();
        if env.schema_version.is_none() {
            env.schema_version = Some(E::CURRENT_SCHEMA_VERSION);
        }
    }
    out
}

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

    /// Current wire-schema version for this resource's `Record` type.
    ///
    /// Bumped when the meaning of an existing field changes (a rare,
    /// deliberate event). Additive field changes don't require a
    /// version bump — they ride on the additive-only policy
    /// (see `wavekat-voice/docs/21-platform-call-history-sync.md`
    /// §"Versioning and forward compatibility").
    ///
    /// Used by `Client::sync` so consumers don't manage the version
    /// themselves — upgrading the bridge crate picks up the right
    /// number automatically. Default is `1`.
    const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// One row's worth of data. Must round-trip through JSON; the wire
    /// shape uses camelCase per the platform's Hono/Zod convention
    /// (apply `#[serde(rename_all = "camelCase")]` on your struct).
    ///
    /// Records must embed [`SyncEnvelope`] via
    /// `#[serde(flatten)] pub envelope: SyncEnvelope` so the
    /// `schemaVersion` + `extras` fields appear at the top of the
    /// JSON object alongside the resource-specific columns. The
    /// trait doesn't enforce this via an associated type because
    /// `#[serde(flatten)]` is a serde attribute (not a Rust trait
    /// bound), but every record type ships with the envelope and
    /// `Client::sync` relies on the field name `schemaVersion`.
    /// See `VoiceCallRecord` for the canonical shape.
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
    ///
    /// **Schema version.** Records whose envelope leaves
    /// `schemaVersion` unset (the common case — consumers don't need
    /// to know the number) have it stamped with
    /// [`SyncEndpoint::CURRENT_SCHEMA_VERSION`] before serialization,
    /// so the platform always sees an explicit version. Records that
    /// set it explicitly are passed through untouched.
    pub async fn sync<E: SyncEndpoint>(&self, items: &[E::Record]) -> Result<SyncResponse>
    where
        E::Record: Clone + HasSyncEnvelope,
    {
        let path = format!("/api/voice/{}/sync", E::RESOURCE);
        let body = SyncRequest {
            items: stamp_schema_version::<E>(items),
        };
        self.post_json::<SyncResponse, _>(&path, &body).await
    }

    /// `GET /api/voice/{E::RESOURCE}` — one page of the caller's rows,
    /// newest first, scoped server-side to the bearer's user.
    pub async fn list<E: SyncEndpoint>(&self, query: &E::Query) -> Result<Page<E::Record>> {
        let path = format!("/api/voice/{}", E::RESOURCE);
        self.get_json_query::<Page<E::Record>, _>(&path, query)
            .await
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
        assert!(
            s.contains("\"sourceId\":\"a\""),
            "wire should be camelCase: {s}"
        );
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

    // A record that carries the envelope via flatten — exactly the
    // shape every real resource type adopts. Verifies the
    // stamp-on-send behaviour without depending on `VoiceCalls`
    // (which lives in a sibling module).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DummyEnvelopedRecord {
        source_id: String,
        #[serde(flatten, default)]
        envelope: SyncEnvelope,
    }

    impl HasSyncEnvelope for DummyEnvelopedRecord {
        fn envelope_mut(&mut self) -> &mut SyncEnvelope {
            &mut self.envelope
        }
    }

    struct EnvelopedResource;
    impl SyncEndpoint for EnvelopedResource {
        const RESOURCE: &'static str = "enveloped";
        const CURRENT_SCHEMA_VERSION: u32 = 7;
        type Record = DummyEnvelopedRecord;
        type Query = DummyQuery;
    }

    #[test]
    fn stamp_schema_version_fills_in_when_missing() {
        let items = vec![DummyEnvelopedRecord {
            source_id: "a".into(),
            envelope: SyncEnvelope::default(),
        }];
        let stamped = stamp_schema_version::<EnvelopedResource>(&items);
        assert_eq!(stamped[0].envelope.schema_version, Some(7));
    }

    #[test]
    fn stamp_schema_version_preserves_explicit_value() {
        // A consumer that deliberately set a version (e.g. a rollback
        // test) should pass through unchanged.
        let items = vec![DummyEnvelopedRecord {
            source_id: "a".into(),
            envelope: SyncEnvelope {
                schema_version: Some(2),
                extras: None,
            },
        }];
        let stamped = stamp_schema_version::<EnvelopedResource>(&items);
        assert_eq!(stamped[0].envelope.schema_version, Some(2));
    }

    #[test]
    fn for_endpoint_returns_envelope_with_current_version() {
        let env = SyncEnvelope::for_endpoint::<EnvelopedResource>();
        assert_eq!(env.schema_version, Some(7));
        assert!(env.extras.is_none());
    }
}
