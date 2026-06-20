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

use crate::client::Client;
use crate::error::{Error, Result};
use crate::sign::ReleaseCredential;
use crate::sync::{stamp_schema_version, HasSyncEnvelope, SyncEndpoint, SyncEnvelope, SyncRequest};

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
    /// An established call torn down because its connection died —
    /// the daemon's RFC 4028 session keepalive stopped getting
    /// answers (peer crashed, NAT binding dropped). Distinct from
    /// `HangupLocal`: the user didn't end this call. Rows with this
    /// reason still carry [`VoiceCallDisposition::Answered`].
    ConnectionLost,
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
    /// Version + forward-compat fields shared by every sync record.
    /// Flattened so `schemaVersion` and `extras` sit at the top of
    /// the JSON object alongside the other columns. See
    /// [`SyncEnvelope`] and doc 21 §"Versioning and forward
    /// compatibility".
    #[serde(flatten, default)]
    pub envelope: SyncEnvelope,
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

impl HasSyncEnvelope for VoiceCallRecord {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope {
        &mut self.envelope
    }
}

// ---- VoiceRecordings ------------------------------------------------------

/// One per-call recording's metadata as it crosses the wire from the
/// daemon up to the platform. The WAV bytes ride on a separate
/// follow-up call ([`Client::upload_recording_bytes`]) so the
/// idempotent metadata sync stays small and a flaky bytes upload
/// doesn't force the daemon to re-ship the row.
///
/// Mirrors the daemon's `RecordingArtifact` (see
/// `wavekat-voice/crates/wavekat-voice/src/recording.rs`) with one
/// rename: the daemon's local id (`id`) ships as `source_id` because
/// the platform allocates its own row id and treats the daemon-side
/// UUID as the idempotency key (same convention as
/// [`VoiceCallRecord`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRecordingRecord {
    /// Daemon-generated UUID for this recording artifact. Upsert key
    /// on the platform side.
    pub source_id: String,
    /// Daemon's `calls.id` — the call this recording belongs to.
    /// The platform stores both so the /voice/calls history page can
    /// link a call to its recording without a separate join table.
    pub call_source_id: String,
    /// Byte length of the WAV file the daemon will PUT in the follow-
    /// up bytes call. The platform refuses a PUT whose body length
    /// disagrees.
    pub size_bytes: u64,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    /// RFC 3339 timestamp the daemon stamped on the artifact at
    /// finalize time. Drives the platform's `/voice/recordings` GET
    /// cursor.
    pub created_at: String,
    #[serde(flatten, default)]
    pub envelope: SyncEnvelope,
}

/// Query params for `GET /api/voice/recordings`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRecordingsQuery {
    /// RFC 3339 cursor; rows with `created_at < before` are returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Marker for the `/api/voice/recordings/{sync,list}` endpoint pair.
///
/// The corresponding bytes-upload endpoint
/// (`PUT /api/voice/recordings/{sourceId}/bytes`) is invoked via
/// [`Client::upload_recording_bytes`] — it doesn't fit the
/// `SyncEndpoint` mold (no batch, no JSON body) so it has its own
/// inherent method on `Client`.
pub struct VoiceRecordings;

impl SyncEndpoint for VoiceRecordings {
    const RESOURCE: &'static str = "recordings";
    type Record = VoiceRecordingRecord;
    type Query = VoiceRecordingsQuery;
}

impl HasSyncEnvelope for VoiceRecordingRecord {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope {
        &mut self.envelope
    }
}

/// One item in the platform's response to
/// `POST /api/voice/recordings/sync`. Lets the daemon learn the R2
/// key the platform stamped (so a subsequent bytes PUT can target it)
/// without re-deriving it, and check whether bytes have already
/// landed on a prior cycle (so the daemon can mark the local row
/// synced without re-uploading the WAV).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRecordingSyncItem {
    pub source_id: String,
    pub r2_key: String,
    pub bytes_uploaded: bool,
}

/// Full response from `POST /api/voice/recordings/sync`. Superset of
/// the generic [`crate::SyncResponse`] — see [`Client::sync_recordings`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceRecordingsSyncResponse {
    pub accepted: u32,
    pub skipped: u32,
    pub items: Vec<VoiceRecordingSyncItem>,
}

// ---- VoiceTranscripts -----------------------------------------------------

/// Wire-stable transcript channel tag. Matches the daemon's
/// `TranscriptChannelLabel` and `events::TranscriptChannel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTranscriptChannel {
    /// Local mic audio — what the user said.
    Local,
    /// Received RTP audio — what the remote party said.
    Remote,
}

/// One ASR transcript segment ("final" in wavekat-asr parlance) as it
/// crosses the wire. Each segment is a row on the daemon side
/// (`transcripts` table); the daemon batches a slice of them per
/// upload and the platform upserts per (user_id, source_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptRecord {
    /// Daemon-side row id, formatted as text (the column is an
    /// autoincrement integer on SQLite). Stable per (call, segment)
    /// so re-shipping converges.
    pub source_id: String,
    /// Daemon's `calls.id` — the call this segment belongs to.
    pub call_source_id: String,
    pub channel: VoiceTranscriptChannel,
    /// Start of the segment in milliseconds relative to the start of
    /// the call's audio stream (not wall-clock).
    pub ts_ms: i64,
    /// End of the segment, same reference frame as `ts_ms`.
    pub end_ms: i64,
    /// Recognised text. Free-form; the platform stores it verbatim.
    pub text: String,
    #[serde(flatten, default)]
    pub envelope: SyncEnvelope,
}

/// Query params for `GET /api/voice/transcripts` — required
/// `call_source_id` (the endpoint refuses a flat list).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscriptsQuery {
    pub call_source_id: String,
}

/// Marker for the `/api/voice/transcripts/{sync,list}` endpoint pair.
pub struct VoiceTranscripts;

impl SyncEndpoint for VoiceTranscripts {
    const RESOURCE: &'static str = "transcripts";
    type Record = VoiceTranscriptRecord;
    type Query = VoiceTranscriptsQuery;
}

impl HasSyncEnvelope for VoiceTranscriptRecord {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope {
        &mut self.envelope
    }
}

// ---- VoiceAccounts --------------------------------------------------------

/// SIP transport for a synced account line. Wire-stable snake_case;
/// mirrors the daemon's `TransportKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceTransport {
    Udp,
    Tcp,
}

/// One SIP account line's *configuration* as it crosses the wire from a
/// device up to the platform and back down to another device
/// (`wavekat-voice/docs/40-account-config-sync.md`).
///
/// Unlike calls / recordings / transcripts — which are immutable,
/// one-way pushes — account config is **mutable and bidirectional**: a
/// line is edited, toggled, renamed, and deleted, and those changes must
/// restore onto a second device. The same idempotent
/// `(user_id, source_id)` upsert that [`Client::sync`] performs carries
/// every kind of change here; a *delete* is a soft-delete that rides as
/// an upsert with `deleted_at` set, because a hard DELETE can't sync
/// under a "push the row" model — once the row is gone there's nothing
/// left to push.
///
/// **No secret field, by construction.** The SIP password never appears
/// on this wire. Config sync (policy levels 1–2) keeps the credential
/// device-local, and the end-to-end-encrypted secret path (level 3)
/// ships its ciphertext through a *separate* opaque resource, never as a
/// field here. Omitting it means level 3 can't be populated by accident
/// before it exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAccountRecord {
    /// Daemon-side account UUID (`accounts.id`). The platform's
    /// `(user_id, source_id)` upsert key — re-syncing the same id
    /// updates the row in place (mutable), unlike the immutable
    /// resources where a re-sync is a no-op.
    pub source_id: String,
    /// Whether the line registers on daemon boot. Pausing a line is a
    /// portable preference, so it rides along.
    pub enabled: bool,
    pub display_name: String,
    pub username: String,
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub transport: VoiceTransport,
    pub register_expires: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive_secs: Option<u32>,
    /// Record-disclosure beep toggle — a column on the account row, so
    /// it rides along for free (the account-portable taxonomy in doc 40).
    pub disclosure_enabled: bool,
    /// RFC 3339 last-modification time — the **last-write-wins key**. On
    /// conflict the platform (and a pulling client) keep the copy with
    /// the later `updated_at`. Whole-row LWW for v1; per-field merge is
    /// deferred until users actually report lost edits (doc 40).
    pub updated_at: String,
    /// RFC 3339 soft-delete tombstone. `None` = live; `Some` = the line
    /// was deleted on some device at that time. A tombstone syncs like
    /// any other mutation so the delete propagates to other devices,
    /// then is reaped locally once confirmed. The platform retains
    /// tombstones so a late-syncing device still learns about the delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    /// Version + forward-compat fields shared by every sync record.
    #[serde(flatten, default)]
    pub envelope: SyncEnvelope,
}

/// Query params for `GET /api/voice/accounts`. All fields optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAccountsQuery {
    /// Include soft-deleted tombstones in the response. Absent / false
    /// returns only live lines — the restore-grade pull a fresh device
    /// wants. A delta-syncing device sets this `true` to also learn
    /// about deletes made elsewhere (doc 40).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_deleted: Option<bool>,
}

/// Marker for the `/api/voice/accounts/{sync,list}` endpoint pair.
///
/// Accounts are the first *mutable, bidirectional* sync resource, but
/// the wire shape is the same idempotent upsert the immutable resources
/// use — the [`SyncResponse::skipped`](crate::sync::SyncResponse) field
/// was reserved for exactly this case — so no new HTTP plumbing is
/// needed: `client.sync::<VoiceAccounts>(&items)` uploads (including
/// tombstones), `client.list::<VoiceAccounts>(&query)` pulls.
pub struct VoiceAccounts;

impl SyncEndpoint for VoiceAccounts {
    const RESOURCE: &'static str = "accounts";
    type Record = VoiceAccountRecord;
    type Query = VoiceAccountsQuery;
}

impl HasSyncEnvelope for VoiceAccountRecord {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope {
        &mut self.envelope
    }
}

// ---- Anonymous install heartbeat ------------------------------------------
//
// A first-run / per-launch ping the desktop daemon fires *before* (and
// independently of) any platform sign-in, so the platform can count
// installs and track version / OS adoption for users who never sign in.
// It hits the public, unauthenticated `POST /api/voice/installs/heartbeat`
// and upserts a row keyed by `install_id` alone (no user) — distinct
// from the authenticated `voice_clients` heartbeat, which is keyed by
// `(user, install_id)`.
//
// The environment fields (os / os_version / arch / locale) are gathered
// *here*, inside the client crate, rather than on the consumer side:
// the daemon only owns the two values this crate genuinely cannot
// discover — the persisted `install_id` and its own app version.

/// Best-effort snapshot of the host environment, detected at call time.
/// Every field is best-effort; a probe that fails contributes `None`
/// (or, for the always-available `os` / `arch`, the compile-time
/// target) rather than failing the heartbeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    /// `std::env::consts::OS` — `"macos"`, `"windows"`, `"linux"`, …
    pub os: String,
    /// Human OS version, e.g. `"15.5.0"`. `None` when the OS probe
    /// can't determine it.
    pub os_version: Option<String>,
    /// `std::env::consts::ARCH` — `"aarch64"`, `"x86_64"`, …
    pub arch: String,
    /// BCP-47 system locale, e.g. `"en-NZ"`. `None` when unset /
    /// undetectable (common for GUI-launched apps on some platforms).
    pub locale: Option<String>,
}

impl SystemInfo {
    /// Probe the current host. Cheap enough to call per heartbeat; we
    /// don't cache so a locale change between launches is reflected.
    pub fn detect() -> Self {
        let os_version = match os_info::get().version() {
            os_info::Version::Unknown => None,
            v => Some(v.to_string()),
        };
        SystemInfo {
            os: std::env::consts::OS.to_string(),
            os_version,
            arch: std::env::consts::ARCH.to_string(),
            locale: sys_locale::get_locale(),
        }
    }
}

/// Body of `POST /api/voice/installs/heartbeat`. The daemon supplies
/// `install_id` + `app_version`; [`Client::install_heartbeat`] fills the
/// environment fields from [`SystemInfo::detect`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallHeartbeatRequest {
    /// The daemon's persisted install UUID — the platform's upsert key.
    pub install_id: String,
    /// WaveKat Voice's own version (`env!("CARGO_PKG_VERSION")` on the
    /// daemon side) — *not* this crate's version.
    pub app_version: String,
    pub os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

/// The platform's view of an install row, echoed back from a heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallHeartbeatResponse {
    pub id: String,
    pub install_id: String,
    pub app_version: String,
    pub os: String,
    pub os_version: Option<String>,
    pub arch: Option<String>,
    pub locale: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

impl Client {
    /// `POST /api/voice/installs/heartbeat` — the anonymous, no-auth
    /// first-run install ping. Detects the host environment internally
    /// and posts it alongside the caller-supplied `install_id` +
    /// `app_version`. Associated (not a method) because the endpoint is
    /// unauthenticated — there's no token, and at first run there's no
    /// signed-in `Client` to hang it off of.
    ///
    /// Though unauthenticated, the request is **signed** with the release
    /// credential `cred` (a per-version Ed25519 key + master-issued
    /// certificate the consumer bakes in at build time) so the platform
    /// can verify it came from a genuine release and reject forged or
    /// replayed pings — see [`Client::post_public_signed_json`] and
    /// [`crate::sign`]. The platform needs only the master *public* key to
    /// verify.
    ///
    /// `base_url` is the platform base (e.g. `https://platform.wavekat.com`).
    pub async fn install_heartbeat(
        base_url: &str,
        install_id: &str,
        app_version: &str,
        cred: &ReleaseCredential,
    ) -> Result<InstallHeartbeatResponse> {
        let sys = SystemInfo::detect();
        let body = InstallHeartbeatRequest {
            install_id: install_id.to_string(),
            app_version: app_version.to_string(),
            os: sys.os,
            os_version: sys.os_version,
            arch: Some(sys.arch),
            locale: sys.locale,
        };
        Client::post_public_signed_json::<InstallHeartbeatResponse, _>(
            base_url,
            "/api/voice/installs/heartbeat",
            &body,
            cred,
        )
        .await
    }
}

// ---- Client surface for recordings ----------------------------------------
//
// Recordings don't fit the generic `Client::sync` shape cleanly:
//
//   - the response carries per-item provenance (the platform-stamped
//     `r2Key`, plus whether bytes have already landed) that the
//     daemon needs in order to decide which rows still owe a PUT;
//   - the bytes upload is its own HTTP call (`PUT
//     /api/voice/recordings/{sourceId}/bytes`), not a JSON batch.
//
// Rather than overloading `SyncEndpoint` to carry these shapes, we
// expose two inherent methods on `Client` that compose the existing
// JSON / bytes-PUT primitives.

impl Client {
    /// `POST /api/voice/recordings/sync` — idempotent batch upsert of
    /// recording metadata. Returns the per-item `r2Key` the daemon
    /// should target for the follow-up bytes PUT, and whether bytes
    /// have already landed for each row.
    ///
    /// Batch sizing rules match [`Client::sync`]: the platform rejects
    /// batches over 100 items; the daemon's uploader chunks at 50.
    pub async fn sync_recordings(
        &self,
        items: &[VoiceRecordingRecord],
    ) -> Result<VoiceRecordingsSyncResponse> {
        let stamped = stamp_schema_version::<VoiceRecordings>(items);
        let body = SyncRequest { items: stamped };
        self.post_json::<VoiceRecordingsSyncResponse, _>("/api/voice/recordings/sync", &body)
            .await
    }

    /// `PUT /api/voice/recordings/{sourceId}/bytes` — upload the WAV
    /// bytes for a recording whose metadata was previously synced via
    /// [`Client::sync_recordings`]. The platform refuses (`HTTP 413`)
    /// if `bytes.len()` disagrees with the synced `sizeBytes`.
    ///
    /// `source_id` is path-segmented as-is; callers pass the
    /// daemon-side UUID they used for the metadata sync. Empty /
    /// path-traversal-shaped ids are not specifically guarded here —
    /// the platform's Zod schema rejects them server-side, so a
    /// malformed id surfaces as a 4xx via [`Error::Http`].
    pub async fn upload_recording_bytes(&self, source_id: &str, bytes: Vec<u8>) -> Result<()> {
        if source_id.is_empty() {
            return Err(Error::BadRequest("source_id must not be empty".into()));
        }
        let path = format!("/api/voice/recordings/{source_id}/bytes");
        self.put_raw_bytes(&path, "audio/wav", bytes).await
    }
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
            envelope: SyncEnvelope::for_endpoint::<VoiceCalls>(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"sourceId\":"), "{s}");
        assert!(s.contains("\"accountId\":"), "{s}");
        assert!(s.contains("\"ringAt\":"), "{s}");
        assert!(s.contains("\"endAt\":"), "{s}");
        assert!(s.contains("\"durationMs\":55000"), "{s}");
        // Optional `error` is None — should be omitted from the wire.
        assert!(!s.contains("\"error\""), "error should be omitted: {s}");
        // Envelope flattens to the top of the object — schemaVersion
        // sits next to the other fields rather than nested under
        // "envelope". Future resources rely on this layout.
        assert!(
            s.contains("\"schemaVersion\":1"),
            "schemaVersion should flatten: {s}"
        );
        // `extras` is None, so the envelope contributes no `extras`
        // key. Stays out of the row to keep the small/fast path.
        assert!(!s.contains("\"extras\""), "extras should be omitted: {s}");
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
        assert_eq!(
            s, "{}",
            "default query should serialize to empty object: {s}"
        );
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
            VoiceCallEndReason::ConnectionLost,
            VoiceCallEndReason::Failed,
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: VoiceCallEndReason = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn connection_lost_pins_its_wire_string() {
        // The platform's sync endpoint validates end reasons against
        // an exact string list — a rename here would make every
        // upload from a session-timer teardown bounce with a 400.
        let s = serde_json::to_string(&VoiceCallEndReason::ConnectionLost).unwrap();
        assert_eq!(s, "\"connection_lost\"");
    }

    #[test]
    fn voice_calls_marker_resource_is_calls() {
        assert_eq!(<VoiceCalls as SyncEndpoint>::RESOURCE, "calls");
    }

    #[test]
    fn record_accepts_unknown_extras_for_forward_compat() {
        // A newer client shipping a `notes` field that this platform
        // version doesn't have a column for should round-trip via
        // the `extras` envelope. The platform persists the blob
        // verbatim; a future deploy can promote it to a typed
        // column without data loss.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "anon",
            "ringAt": "2026-05-16T10:00:00Z",
            "endAt": "2026-05-16T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_remote",
            "schemaVersion": 2,
            "extras": { "notes": "from staging build" }
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.envelope.schema_version, Some(2));
        let extras = parsed.envelope.extras.as_ref().expect("extras present");
        assert_eq!(extras["notes"], "from staging build");
    }

    #[test]
    fn recording_marker_resource_is_recordings() {
        // Path constant drives the URL in `Client::sync_recordings`;
        // a rename here would silently 404 against the platform.
        assert_eq!(<VoiceRecordings as SyncEndpoint>::RESOURCE, "recordings");
    }

    #[test]
    fn recording_record_serializes_with_camel_case_and_envelope() {
        let r = VoiceRecordingRecord {
            source_id: "11111111-1111-4111-8111-111111111111".into(),
            call_source_id: "22222222-2222-4222-8222-222222222222".into(),
            size_bytes: 44 + 64_000,
            duration_ms: 2_000,
            sample_rate: 8_000,
            channels: 2,
            created_at: "2026-05-16T10:01:05Z".into(),
            envelope: SyncEnvelope::for_endpoint::<VoiceRecordings>(),
        };
        let s = serde_json::to_string(&r).unwrap();
        // Field-by-field wire contract — these strings are also what
        // the platform's Zod schema expects.
        assert!(s.contains("\"sourceId\":"), "{s}");
        assert!(s.contains("\"callSourceId\":"), "{s}");
        assert!(s.contains("\"sizeBytes\":64044"), "{s}");
        assert!(s.contains("\"durationMs\":2000"), "{s}");
        assert!(s.contains("\"sampleRate\":8000"), "{s}");
        assert!(s.contains("\"channels\":2"), "{s}");
        assert!(s.contains("\"createdAt\":"), "{s}");
        // Envelope flattens to the top of the object, same as VoiceCallRecord.
        assert!(s.contains("\"schemaVersion\":1"), "{s}");
    }

    #[test]
    fn recordings_sync_response_round_trips() {
        // The richer-than-generic response carries per-item provenance —
        // the daemon's uploader reads `r2Key` for the bytes follow-up
        // and `bytesUploaded` to short-circuit when the row already
        // landed on a previous cycle.
        let raw = r#"{
            "accepted": 2,
            "skipped": 0,
            "items": [
                {"sourceId": "a", "r2Key": "voice/recordings/1/a.wav", "bytesUploaded": false},
                {"sourceId": "b", "r2Key": "voice/recordings/1/b.wav", "bytesUploaded": true}
            ]
        }"#;
        let parsed: VoiceRecordingsSyncResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.accepted, 2);
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.items[0].r2_key, "voice/recordings/1/a.wav");
        assert!(!parsed.items[0].bytes_uploaded);
        assert!(parsed.items[1].bytes_uploaded);
    }

    #[test]
    fn install_heartbeat_request_serializes_with_camel_case_keys() {
        let req = InstallHeartbeatRequest {
            install_id: "11111111-1111-4111-8111-111111111111".into(),
            app_version: "0.0.21".into(),
            os: "macos".into(),
            os_version: Some("15.5.0".into()),
            arch: Some("aarch64".into()),
            locale: Some("en-NZ".into()),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"installId\":"), "{s}");
        assert!(s.contains("\"appVersion\":\"0.0.21\""), "{s}");
        assert!(s.contains("\"os\":\"macos\""), "{s}");
        assert!(s.contains("\"osVersion\":\"15.5.0\""), "{s}");
        assert!(s.contains("\"arch\":\"aarch64\""), "{s}");
        assert!(s.contains("\"locale\":\"en-NZ\""), "{s}");
    }

    #[test]
    fn install_heartbeat_request_omits_absent_optional_fields() {
        // A host where the OS version / locale probe came up empty
        // shouldn't send `null` — keeping the keys out lets the
        // platform's Zod `.optional()` accept the body and the column
        // stay NULL rather than the string "null".
        let req = InstallHeartbeatRequest {
            install_id: "x".into(),
            app_version: "0.0.21".into(),
            os: "linux".into(),
            os_version: None,
            arch: None,
            locale: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("osVersion"), "osVersion should be omitted: {s}");
        assert!(!s.contains("arch"), "arch should be omitted: {s}");
        assert!(!s.contains("locale"), "locale should be omitted: {s}");
    }

    #[test]
    fn install_heartbeat_response_parses_platform_shape() {
        let raw = r#"{
            "id": "abc-123",
            "installId": "11111111-1111-4111-8111-111111111111",
            "appVersion": "0.0.21",
            "os": "macos",
            "osVersion": "15.5.0",
            "arch": "aarch64",
            "locale": null,
            "firstSeenAt": "2026-05-31T10:00:00.000Z",
            "lastSeenAt": "2026-05-31T10:00:00.000Z"
        }"#;
        let parsed: InstallHeartbeatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.id, "abc-123");
        assert_eq!(parsed.app_version, "0.0.21");
        assert_eq!(parsed.os_version.as_deref(), Some("15.5.0"));
        assert!(parsed.locale.is_none());
    }

    #[test]
    fn system_info_detect_fills_os_and_arch() {
        // os / arch come from compile-time consts, so they're always
        // non-empty on every supported target. os_version / locale are
        // best-effort and intentionally not asserted.
        let sys = SystemInfo::detect();
        assert!(!sys.os.is_empty(), "os should be a non-empty target string");
        assert!(
            !sys.arch.is_empty(),
            "arch should be a non-empty target string"
        );
    }

    #[test]
    fn transcripts_marker_resource_is_transcripts() {
        assert_eq!(<VoiceTranscripts as SyncEndpoint>::RESOURCE, "transcripts");
    }

    #[test]
    fn transcript_record_serializes_with_camel_case_and_channel_enum() {
        let r = VoiceTranscriptRecord {
            source_id: "1".into(),
            call_source_id: "22222222-2222-4222-8222-222222222222".into(),
            channel: VoiceTranscriptChannel::Remote,
            ts_ms: 100,
            end_ms: 1_500,
            text: "hello".into(),
            envelope: SyncEnvelope::for_endpoint::<VoiceTranscripts>(),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"sourceId\":"), "{s}");
        assert!(s.contains("\"callSourceId\":"), "{s}");
        // The channel enum is wire-stable snake_case — matches the
        // platform's Zod `enum(VOICE_TRANSCRIPT_CHANNELS)`.
        assert!(s.contains("\"channel\":\"remote\""), "{s}");
        assert!(s.contains("\"tsMs\":100"), "{s}");
        assert!(s.contains("\"endMs\":1500"), "{s}");
        assert!(s.contains("\"text\":\"hello\""), "{s}");
        assert!(s.contains("\"schemaVersion\":1"), "{s}");
    }

    // ---- VoiceAccounts ----

    fn sample_account() -> VoiceAccountRecord {
        VoiceAccountRecord {
            source_id: "11111111-1111-4111-8111-111111111111".into(),
            enabled: true,
            display_name: "Work line".into(),
            username: "alice".into(),
            domain: "sip.example.com".into(),
            auth_username: Some("alice-auth".into()),
            server: Some("sip.example.com".into()),
            port: Some(5060),
            transport: VoiceTransport::Udp,
            register_expires: 60,
            keepalive_secs: Some(50),
            disclosure_enabled: true,
            updated_at: "2026-06-20T10:00:00Z".into(),
            deleted_at: None,
            envelope: SyncEnvelope::for_endpoint::<VoiceAccounts>(),
        }
    }

    #[test]
    fn accounts_marker_resource_is_accounts() {
        // Path constant drives the URL in `Client::sync` / `Client::list`;
        // a rename here would silently 404 against the platform.
        assert_eq!(<VoiceAccounts as SyncEndpoint>::RESOURCE, "accounts");
    }

    #[test]
    fn account_record_serializes_with_camel_case_and_envelope() {
        let s = serde_json::to_string(&sample_account()).unwrap();
        // Field-by-field wire contract — also what the platform's Zod
        // schema expects.
        assert!(s.contains("\"sourceId\":"), "{s}");
        assert!(s.contains("\"displayName\":\"Work line\""), "{s}");
        assert!(s.contains("\"authUsername\":\"alice-auth\""), "{s}");
        assert!(s.contains("\"registerExpires\":60"), "{s}");
        assert!(s.contains("\"keepaliveSecs\":50"), "{s}");
        assert!(s.contains("\"disclosureEnabled\":true"), "{s}");
        assert!(s.contains("\"transport\":\"udp\""), "{s}");
        assert!(s.contains("\"updatedAt\":\"2026-06-20T10:00:00Z\""), "{s}");
        // A live line carries no tombstone.
        assert!(!s.contains("deletedAt"), "deletedAt should be omitted: {s}");
        // The secret never crosses this wire, by construction.
        assert!(!s.contains("password"), "no password field: {s}");
        // Envelope flattens to the top, same as the other resources.
        assert!(s.contains("\"schemaVersion\":1"), "{s}");
    }

    #[test]
    fn account_tombstone_serializes_deleted_at() {
        // A soft-delete rides as an upsert with deletedAt set — the
        // delete-propagation mechanism (doc 40).
        let mut r = sample_account();
        r.deleted_at = Some("2026-06-20T12:00:00Z".into());
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"deletedAt\":\"2026-06-20T12:00:00Z\""), "{s}");
    }

    #[test]
    fn account_record_round_trips_optional_fields() {
        // A minimal line — no auth username, server, port, keepalive, or
        // tombstone — should parse with those all absent.
        let raw = r#"{
            "sourceId": "a",
            "enabled": false,
            "displayName": "Cheap trunk",
            "username": "u",
            "domain": "d",
            "transport": "tcp",
            "registerExpires": 120,
            "disclosureEnabled": false,
            "updatedAt": "2026-06-20T10:00:00Z"
        }"#;
        let parsed: VoiceAccountRecord = serde_json::from_str(raw).unwrap();
        assert!(!parsed.enabled);
        assert!(parsed.auth_username.is_none());
        assert!(parsed.server.is_none());
        assert!(parsed.port.is_none());
        assert!(parsed.keepalive_secs.is_none());
        assert!(parsed.deleted_at.is_none());
        assert_eq!(parsed.transport, VoiceTransport::Tcp);
        assert_eq!(parsed.register_expires, 120);
    }

    #[test]
    fn voice_transport_round_trips_via_json() {
        for t in [VoiceTransport::Udp, VoiceTransport::Tcp] {
            let s = serde_json::to_string(&t).unwrap();
            let back: VoiceTransport = serde_json::from_str(&s).unwrap();
            assert_eq!(t, back);
        }
        // Pin the wire strings — the daemon's `TransportKind` and the
        // platform's Zod enum both depend on these exact tokens.
        assert_eq!(
            serde_json::to_string(&VoiceTransport::Udp).unwrap(),
            "\"udp\""
        );
        assert_eq!(
            serde_json::to_string(&VoiceTransport::Tcp).unwrap(),
            "\"tcp\""
        );
    }

    #[test]
    fn accounts_query_omits_unset_and_serializes_include_deleted() {
        let empty = serde_json::to_string(&VoiceAccountsQuery::default()).unwrap();
        assert_eq!(empty, "{}", "default query should be empty: {empty}");
        let with_deleted = serde_json::to_string(&VoiceAccountsQuery {
            include_deleted: Some(true),
        })
        .unwrap();
        assert!(
            with_deleted.contains("\"includeDeleted\":true"),
            "{with_deleted}"
        );
    }

    #[test]
    fn account_record_accepts_unknown_extras_for_forward_compat() {
        // A newer client shipping a field this platform version lacks a
        // column for round-trips via the `extras` envelope.
        let raw = r#"{
            "sourceId": "a",
            "enabled": true,
            "displayName": "x",
            "username": "u",
            "domain": "d",
            "transport": "udp",
            "registerExpires": 60,
            "disclosureEnabled": true,
            "updatedAt": "2026-06-20T10:00:00Z",
            "schemaVersion": 2,
            "extras": { "ringtone": "classic" }
        }"#;
        let parsed: VoiceAccountRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.envelope.schema_version, Some(2));
        let extras = parsed.envelope.extras.as_ref().expect("extras present");
        assert_eq!(extras["ringtone"], "classic");
    }
}
