//! Voice-product resources synced from the desktop daemon up to the
//! platform.
//!
//! The first shipped marker is [`VoiceCalls`] — per-call metadata for
//! the platform's `/voice/calls` history page (see
//! `wavekat-voice/docs/21-platform-call-history-sync.md`). Recordings
//! (`VoiceRecordings`), transcripts (`VoiceTranscripts`), and summaries
//! will follow the same shape: a marker type, a wire-record struct, and
//! a typed query — no new HTTP plumbing.
//!
//! All wire shapes use camelCase JSON to match the platform's Hono/Zod
//! convention. The Rust types stay snake_case so consumers feel native.

use serde::{Deserialize, Serialize};

use crate::sync::SyncEndpoint;

/// Inbound vs. outbound. Wire-stable snake_case strings — never
/// renumber or rename. New states (e.g. `internal`) would be a wire
/// addition, not a replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallDirection {
    Inbound,
    Outbound,
}

/// User-visible disposition. Derived from [`VoiceCallEndReason`] by the
/// daemon; the platform stores both, so future UI surfaces can read
/// either without re-deriving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallDisposition {
    Answered,
    Missed,
    Rejected,
    Cancelled,
    Failed,
}

/// Finer-grained terminal reason — kept distinct from
/// [`VoiceCallDisposition`] because the disposition collapses
/// `hangup_local` and `hangup_remote` to `Answered`, losing the
/// "who hung up?" answer the row otherwise carries.
///
/// Wire-stable snake_case strings; the daemon's matching enum is the
/// canonical source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallEndReason {
    HangupLocal,
    HangupRemote,
    RejectedLocal,
    RejectedRemote,
    Missed,
    CancelledLocal,
    Failed,
}

/// One historical call as it crosses the wire from the daemon up to the
/// platform.
///
/// Mirrors the daemon's local `CallRecord` (see
/// `wavekat-voice/crates/wavekat-voice/src/db.rs`) with one rename:
/// the daemon's local primary key (`id`) is shipped as `source_id`
/// because the platform allocates its own row id and treats the
/// daemon-side UUID as the idempotency key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallRecord {
    /// Daemon-generated UUID. The platform's `(user_id, source_id)`
    /// upsert key — re-syncing the same id is a no-op.
    pub source_id: String,
    /// SIP account UUID on the daemon side. Opaque to the platform.
    pub account_id: String,
    pub direction: VoiceCallDirection,
    /// SIP `From:` (inbound) or `To:` (outbound). Free text — caller
    /// IDs, display names, and SIP URIs all land here.
    pub party: String,
    /// RFC 3339. First ring (inbound) or first dial-out (outbound).
    pub ring_at: String,
    /// RFC 3339. Present only when the call reached the answered
    /// state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_at: Option<String>,
    /// RFC 3339. Terminal timestamp; the platform uses this as the
    /// list cursor.
    pub end_at: String,
    /// `answer_at` → `end_at` in milliseconds. `None` for calls that
    /// were never answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub disposition: VoiceCallDisposition,
    pub end_reason: VoiceCallEndReason,
    /// Free-text error, populated only when `disposition == Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Query params for `GET /api/voice/calls`. All fields optional — the
/// default returns the newest page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallsQuery {
    /// RFC 3339 cursor; rows with `end_at < before` are returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// 1..=200. Server default is 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Marker for the `/api/voice/calls/{sync,list}` endpoint pair.
///
/// Use as a type parameter, never construct: `client.sync::<VoiceCalls>(&items)`.
pub struct VoiceCalls;

impl SyncEndpoint for VoiceCalls {
    const RESOURCE: &'static str = "calls";
    type Record = VoiceCallRecord;
    type Query = VoiceCallsQuery;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_serializes_with_camel_case_keys() {
        let r = VoiceCallRecord {
            source_id: "11111111-1111-4111-8111-111111111111".into(),
            account_id: "22222222-2222-4222-8222-222222222222".into(),
            direction: VoiceCallDirection::Inbound,
            party: "+14155550123".into(),
            ring_at: "2026-05-16T10:00:00Z".into(),
            answer_at: Some("2026-05-16T10:00:05Z".into()),
            end_at: "2026-05-16T10:01:00Z".into(),
            duration_ms: Some(55_000),
            disposition: VoiceCallDisposition::Answered,
            end_reason: VoiceCallEndReason::HangupRemote,
            error: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"sourceId\":"), "{s}");
        assert!(s.contains("\"accountId\":"), "{s}");
        assert!(s.contains("\"ringAt\":"), "{s}");
        assert!(s.contains("\"endAt\":"), "{s}");
        assert!(s.contains("\"durationMs\":55000"), "{s}");
        // Optional `error` is None — should be omitted from the wire.
        assert!(!s.contains("\"error\""), "error should be omitted: {s}");
    }

    #[test]
    fn record_round_trips_optional_fields() {
        // An unanswered call has answer_at/duration_ms/error all absent.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "anonymous",
            "ringAt": "2026-05-16T10:00:00Z",
            "endAt": "2026-05-16T10:00:30Z",
            "disposition": "missed",
            "endReason": "missed"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert!(parsed.answer_at.is_none());
        assert!(parsed.duration_ms.is_none());
        assert!(parsed.error.is_none());
        assert_eq!(parsed.disposition, VoiceCallDisposition::Missed);
        assert_eq!(parsed.end_reason, VoiceCallEndReason::Missed);
    }

    #[test]
    fn query_omits_unset_fields() {
        let q = VoiceCallsQuery::default();
        let s = serde_json::to_string(&q).unwrap();
        // Empty object — every field skipped when None.
        assert_eq!(s, "{}", "default query should serialize to empty object: {s}");
    }

    #[test]
    fn enum_round_trip_via_json() {
        // The wire form for each direction/disposition/reason must
        // match what the daemon and platform expect — this guards
        // against accidental Rust-side renames.
        for d in [VoiceCallDirection::Inbound, VoiceCallDirection::Outbound] {
            let s = serde_json::to_string(&d).unwrap();
            let back: VoiceCallDirection = serde_json::from_str(&s).unwrap();
            assert_eq!(d, back);
        }
        for d in [
            VoiceCallDisposition::Answered,
            VoiceCallDisposition::Missed,
            VoiceCallDisposition::Rejected,
            VoiceCallDisposition::Cancelled,
            VoiceCallDisposition::Failed,
        ] {
            let s = serde_json::to_string(&d).unwrap();
            let back: VoiceCallDisposition = serde_json::from_str(&s).unwrap();
            assert_eq!(d, back);
        }
        for r in [
            VoiceCallEndReason::HangupLocal,
            VoiceCallEndReason::HangupRemote,
            VoiceCallEndReason::RejectedLocal,
            VoiceCallEndReason::RejectedRemote,
            VoiceCallEndReason::Missed,
            VoiceCallEndReason::CancelledLocal,
            VoiceCallEndReason::Failed,
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: VoiceCallEndReason = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn voice_calls_marker_resource_is_calls() {
        assert_eq!(<VoiceCalls as SyncEndpoint>::RESOURCE, "calls");
    }
}
