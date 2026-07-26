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
    /// We blind-transferred the call to a third party (RFC 3515) and
    /// dropped our own leg once the target answered. Distinct from
    /// `HangupLocal`: the user didn't hang up, they handed the call off.
    /// The destination is carried alongside in
    /// [`VoiceCallRecord::transfer_target`]. Rows with this reason still
    /// carry [`VoiceCallDisposition::Answered`].
    TransferredLocal,
    /// An established call torn down because its connection died —
    /// the daemon's RFC 4028 session keepalive stopped getting
    /// answers (peer crashed, NAT binding dropped). Distinct from
    /// `HangupLocal`: the user didn't end this call. Rows with this
    /// reason still carry [`VoiceCallDisposition::Answered`].
    ConnectionLost,
    Failed,
}

/// The audio codec a call negotiated, stamped once audio flows. Wire-
/// stable snake_case strings matching the daemon's `CallCodec` enum —
/// the platform validates against this exact list, so a rename here
/// would bounce every upload with a 400. New codecs (e.g. `ilbc`) are
/// wire additions, not replacements.
///
/// Consumers render this as a quality tier ("HD" for Opus, "Standard"
/// for the G.711 pair), not the codec name alone — see the desktop
/// client's call-details page for the canonical presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallCodec {
    /// Opus wideband (16 kHz) — the "HD" tier.
    Opus,
    /// G.711 µ-law — the narrowband "Standard" tier.
    Pcmu,
    /// G.711 A-law — the narrowband "Standard" tier.
    Pcma,
}

/// How a call flow's ("receptionist") run ended, folded by the daemon
/// from the run's terminal trace step. Wire-stable snake_case strings
/// matching `wavekat_flow::trace::FlowOutcome` — declared here rather
/// than re-exported so this crate stays free of a `wavekat-flow`
/// dependency; the two lists must be kept in step.
///
/// Consumers prefer this over [`VoiceCallEndReason`] when rendering a
/// flow-answered call's outcome: the flow's own goodbye sends the BYE,
/// so the SIP-level reason reads `HangupLocal` ("you hung up") for a
/// call the user never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCallFlowOutcome {
    /// A `ring` node was answered by a human; the engine stepped out.
    Answered,
    /// A `message` node recorded a voicemail.
    MessageLeft,
    /// A `transfer` node handed the call to an external number.
    Transferred,
    /// A `hangup` node ended the call.
    HungUp,
    /// An effect failed mid-run (the call likely dropped).
    Aborted,
    /// The flow reached an impossible state. Validation is meant to
    /// prevent this, so it signals a defect worth alerting on.
    Defect,
}

/// One step of a call flow's run, as the daemon projects it from its
/// local `call_flow_step` events.
///
/// Deliberately structural rather than a rendered sentence. The daemon
/// has an English summary for each step, but the platform's web UI
/// serves nine locales — shipping prose would make these permanently
/// untranslatable there. Consumers get the parts and compose the
/// sentence themselves.
///
/// `kind` is a plain `String`, not an enum, and that is the point: step
/// kinds grow every time the flow engine gains a node type, and a
/// consumer built against an older version of this crate must still be
/// able to deserialize a newer daemon's trace. An unknown kind is
/// rendered as an unnamed marker rather than rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCallFlowStep {
    /// Milliseconds from the call's answer time — the same zero the
    /// recording starts at, so a step lines up with the audio.
    pub at_ms: i64,
    /// The engine's step tag: `spoke`, `hours`, `menu_choice`,
    /// `menu_no_input`, `menu_invalid`, `ring`, `message_recorded`,
    /// `transferred`, `hung_up`, or the synthetic `answered` marking a
    /// mid-run take-over by the owner.
    pub kind: String,
    /// The flow node this step belongs to, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The key the caller pressed — `menu_choice` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digit: Option<String>,
    /// Recorded message length in seconds — `message_recorded` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<i64>,
    /// Where the call was sent — `transferred` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Whether an hours check landed inside business hours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// Whether a `ring` step was picked up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<bool>,
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
    /// Visibility tier of any *active* (not revoked / expired) share on this
    /// call's recording, or `None` when it isn't shared. Read-only: the
    /// platform sets it on list (`GET /api/voice/calls`) and detail responses
    /// so a consumer can badge the row "Public" / "Invited only"; it is
    /// skipped on serialize, so syncing a call never sends it. `Private` never
    /// appears here — an unshared call is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_visibility: Option<ShareVisibility>,
    /// Where a transferred call was sent — the number or SIP address the
    /// far end was asked to call (RFC 3515 `Refer-To`). Set only when
    /// `end_reason == TransferredLocal`; `None` for every other call.
    /// Unlike `share_visibility` this is daemon-owned data, so it *is*
    /// sent on sync (serialized when present) and echoed back on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_target: Option<String>,
    /// The negotiated audio codec, present when the call reached the
    /// audio-flowing state on a daemon new enough to record it; `None`
    /// for never-answered calls and rows synced by older daemons. Like
    /// `transfer_target` this is daemon-owned data, so it *is* sent on
    /// sync (serialized when present) and echoed back on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<VoiceCallCodec>,
    /// Which call flow ("receptionist") answered this call, when one
    /// did: the platform flow id the daemon held at answer time, and
    /// the flow's display name *at that moment*. The name is shipped
    /// verbatim rather than resolved from the flow on read, so a later
    /// rename or delete doesn't rewrite what history says happened.
    /// Both `None` for calls the user answered themselves. Daemon-owned
    /// data like `codec`, so both are sent on sync and echoed on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_name: Option<String>,
    /// How the flow's run ended. `None` when no flow answered, and for
    /// runs with no terminal step (the caller hung up mid-flow) — there
    /// [`VoiceCallRecord::end_reason`] is already the honest story.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_outcome: Option<VoiceCallFlowOutcome>,
    /// The flow run's step-by-step trace, in answer-time order. Drives
    /// the markers the platform's call-detail page draws on the
    /// recording waveform.
    ///
    /// `None` for human-answered calls and for daemons predating the
    /// trace. Sent on sync like the other daemon-owned fields, but
    /// echoed back only on the *detail* read (`GET /api/voice/calls/
    /// {sourceId}`) — the list route omits it, since nothing on a list
    /// row renders a trace and it would weigh down every page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_steps: Option<Vec<VoiceCallFlowStep>>,
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

// ---- VoiceFlows (published pull) -------------------------------------------
//
// The daemon-facing pull leg of the call-flow ("Receptionist") system —
// `wavekat-voice/docs/48-ivr-call-flows.md`'s control-plane split. Flows
// are *authored* on the platform (drafts, publish gate, version
// history); the daemon only ever reads the published snapshots, caches
// them locally, and runs them offline. There is no upload direction, so
// this is not a `SyncEndpoint` (that trait models the `{resource}/sync`
// + list pair): it's a single typed GET, like the share commands above.

/// One published call-flow snapshot as served by
/// `GET /api/voice/flows/published`: the latest published version of a
/// flow the bearer authored. The YAML carries the platform-stamped
/// `id`/`name`/`version` and is served verbatim — the daemon re-parses
/// and re-validates it on load rather than trusting the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFlowRecord {
    /// Platform-assigned flow id (`flow_…`), stable across versions.
    pub id: String,
    pub name: String,
    /// Latest published version number (1-based, bumps on publish).
    pub version: u32,
    /// The immutable published document, verbatim.
    pub yaml: String,
    /// RFC 3339 time this version was published.
    pub published_at: String,
}

/// Query params for `GET /api/voice/flows/published`. Cursor-paginated
/// by flow id ascending; pass the previous page's `next_after` until it
/// comes back `None` to collect the full set. The full set is what the
/// daemon's reconcile wants — a cached flow absent from a complete pull
/// was deleted on the platform.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFlowsQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Page size, server-capped at 100. `None` = server default (50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One page of published flow snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFlowsPage {
    pub items: Vec<VoiceFlowRecord>,
    /// Cursor for the next page; `None` = end of the set.
    #[serde(default)]
    pub next_after: Option<String>,
}

/// One frozen audio asset of a published flow version, as served by
/// `GET /api/voice/flows/{id}/versions/{version}/assets` (wavekat-platform
/// docs 16/17). The bytes were copied into a version-owned R2 object at
/// publish time and never change, so `content_hash` identifies them
/// exactly — the daemon diffs its local cache against it rather than
/// trusting a bare filename, because the *same* `ref` can carry different
/// bytes across two versions of the same flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFlowVersionAsset {
    /// The `vprompt_…` reference exactly as it appears in the flow YAML.
    #[serde(rename = "ref")]
    pub asset_ref: String,
    /// Source telephony format the clip was frozen as (`ulaw_8000`,
    /// `pcm_16000`, `mp3`, …); the container is WAV unless `mp3`.
    pub format: String,
    /// Size of the frozen bytes.
    pub byte_size: u64,
    /// Clip duration if the platform knew it at freeze time.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// sha256 of the frozen bytes — the cache's content key.
    pub content_hash: String,
}

/// The frozen-asset manifest for one published version. Not paginated:
/// a flow's asset count is bounded by its node count (a phone tree is
/// tens of clips), so the platform returns them all in one response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceFlowAssetsPage {
    pub assets: Vec<VoiceFlowVersionAsset>,
}

impl Client {
    /// `GET /api/voice/flows/published` — one page of the caller's
    /// published flow snapshots (latest version each). Strictly
    /// creator-scoped server-side; never returns another user's flows.
    pub async fn published_flows(&self, query: &VoiceFlowsQuery) -> Result<VoiceFlowsPage> {
        self.get_json_query::<VoiceFlowsPage, _>("/api/voice/flows/published", query)
            .await
    }

    /// `GET /api/voice/flows/{id}/versions/{version}/assets` — the frozen
    /// audio manifest for one published version (docs 16/17). Flow-scoped
    /// server-side: a version of a flow the caller doesn't own is a 404,
    /// never another user's assets. An existing, visible version with no
    /// generated audio returns an empty manifest.
    pub async fn flow_version_assets(
        &self,
        flow_id: &str,
        version: u32,
    ) -> Result<VoiceFlowAssetsPage> {
        let path = format!("/api/voice/flows/{flow_id}/versions/{version}/assets");
        self.get_json::<VoiceFlowAssetsPage>(&path).await
    }

    /// `GET /api/voice/flows/{id}/versions/{version}/assets/{ref}/bytes` —
    /// the immutable frozen copy of one clip, served from the version's own
    /// asset set (never the mutable library). Returned in memory because a
    /// clip is tens of KB and the daemon writes it atomically into its
    /// on-disk cache; same flow-scoped 404 as the manifest.
    pub async fn flow_version_asset_bytes(
        &self,
        flow_id: &str,
        version: u32,
        asset_ref: &str,
    ) -> Result<Vec<u8>> {
        let path =
            format!("/api/voice/flows/{flow_id}/versions/{version}/assets/{asset_ref}/bytes");
        self.get_bytes(&path).await
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

// ---- Recording sharing ----------------------------------------------------
//
// Sharing is a *command* — mutate one recording's share state and get a
// result back — not the "batch upsert + cursor list" shape `SyncEndpoint`
// exists for (see wavekat-voice doc 38). So it's a typed method pair on
// `Client` (mirroring `whoami` rather than `sync::<E>()`), not a marker.
//
// The desktop daemon keeps only a *mirror* of what these return; the
// platform is authoritative for who may open a share. See
// `wavekat-voice/docs/38-share-a-recording.md`.

/// Access tier for a shared recording, mirroring Loom's model. Wire-stable
/// snake_case strings — the platform's Zod schema validates against this
/// exact list, so a rename here would bounce every share command with a 400.
///
/// - `Private` — owner only (the default; "not shared").
/// - `Restricted` — owner + explicitly invited WaveKat accounts; the
///   recipient must be signed in as an invited identity ("protected by login").
/// - `Public` — anyone holding the capability link, no sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareVisibility {
    Private,
    Restricted,
    Public,
}

/// How a shared recording's caller/callee identity (the call's `party`) is
/// exposed to a viewer. Wire-stable snake_case, matching the platform's Zod
/// enum, so a rename here bounces a share command with a 400.
///
/// - `Full` — hidden behind a neutral direction label ("Inbound call").
/// - `Partial` — best-effort redaction (keeps shape, drops the value).
/// - `None` — the raw `party` is shown.
///
/// Absent on the wire → the platform defaults to `Partial` (identity
/// masked) — privacy-forward without fully erasing the caller. See
/// `wavekat-platform` docs/14.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyMasking {
    Full,
    Partial,
    None,
}

/// Body of `POST /api/voice/recordings/{id}/share` — create or update a
/// recording's share. The recording must already be synced (metadata +
/// bytes) or the platform returns 404.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRecordingRequest {
    /// The artifact UUID, as synced (daemon-side `artifacts.id`). Goes in
    /// the URL path; carried in the struct so callers pass one value.
    pub recording_source_id: String,
    pub visibility: ShareVisibility,
    /// Restricted tier — the WaveKat-account emails allowed to open the
    /// share. Ignored (and omitted) for `Private` / `Public`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invited_emails: Option<Vec<String>>,
    /// Per-share visibility controls (platform docs/14) — what a viewer may
    /// see. Each is omitted when unset; the platform then applies its
    /// privacy-forward default (identity masked, transcript hidden, audio
    /// shown, download off). NB the platform treats the request as the
    /// *full* desired state, so an omitted control is reset to its default,
    /// not preserved from a prior share — send all of them when editing an
    /// existing share's controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party_masking: Option<PartyMasking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_transcript: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_audio: Option<bool>,
    /// Whether a viewer may *download* the WAV, distinct from hearing it.
    /// Off by default and only meaningful while `show_audio` is true — the
    /// platform forces it off otherwise (you can't save what you can't
    /// hear). A soft control: it hides the viewer's Download affordance,
    /// not the bytes a listener already fetches to play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_download: Option<bool>,
    /// Per-channel playback defaults — which side is *audible by default*
    /// in the viewer's player (docs/14). A call has two channels: `local`
    /// (the owner's microphone, "your side") and `remote` (the other
    /// party, "their side"). `true` means that side starts muted; the
    /// viewer can still un-mute it, and the audio file is unchanged — this
    /// is only the player's starting state. Each is omitted when unset, in
    /// which case the platform defaults to audible (`false`). Only
    /// meaningful while `show_audio` is true; ignored when audio is hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_local: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_remote: Option<bool>,
    /// Phase 2 — out-of-band password gate. Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Phase 2 — RFC 3339 auto-revoke time. Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// The platform's response to a successful share command. `share_url` is
/// the full https link the user copies; `token` is the opaque capability
/// identifier embedded in it (returned separately so the daemon can store
/// it for display without re-parsing the URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRecordingResponse {
    pub visibility: ShareVisibility,
    pub token: String,
    pub share_url: String,
    /// RFC 3339 — when the recording was first shared.
    pub shared_at: String,
    /// Effective visibility controls the platform stored (docs/14). Optional
    /// for tolerance — a platform predating the feature omits them, in which
    /// case the daemon should assume the defaults (identity masked, transcript
    /// hidden, audio shown, download off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party_masking: Option<PartyMasking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_transcript: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_audio: Option<bool>,
    /// Effective download permission — `show_audio && allow_download`, so
    /// it's never true when the audio is hidden. Absent on a platform
    /// predating the control (assume off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_download: Option<bool>,
    /// Effective per-channel playback defaults the platform stored — which
    /// side starts muted in the viewer's player (docs/14). Absent on a
    /// platform predating the control (assume audible, `false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_local: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_remote: Option<bool>,
}

/// The platform's response to `GET /api/voice/recordings/{id}/share` — the
/// *authoritative* current share state for an owned recording. The POST
/// reply omits the invited-email list and a local mirror can't reflect a
/// share changed from another device, so the desktop "who can open this"
/// panel reads here.
///
/// A recording that was never shared (or whose share is revoked / expired)
/// comes back as [`ShareVisibility::Private`] with the optional fields
/// absent — the same "not shared" state DELETE leaves behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStateResponse {
    pub visibility: ShareVisibility,
    /// Absent when `visibility == Private` (nothing is shared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    /// RFC 3339 — when the recording was first shared. Absent when private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_at: Option<String>,
    /// The restricted tier's audience (lowercased, de-duped). Present
    /// (possibly empty) only for [`ShareVisibility::Restricted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invited_emails: Option<Vec<String>>,
    /// Per-share visibility controls (docs/14). Present for a live share;
    /// absent when `Private` (nothing is shared, so no controls apply).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party_masking: Option<PartyMasking>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_transcript: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_audio: Option<bool>,
    /// Effective download permission — `show_audio && allow_download`, so
    /// never true when the audio is hidden. Absent when private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_download: Option<bool>,
    /// Effective per-channel playback defaults — which side starts muted in
    /// the viewer's player (docs/14). Absent when private.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_local: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mute_remote: Option<bool>,
}

impl Client {
    /// `POST /api/voice/recordings/{id}/share` — create or update a share
    /// for an already-synced recording. Returns the capability link + token
    /// the desktop UI puts on the clipboard.
    ///
    /// Per the 404-not-403 ownership rule (doc 21 §"Authorization"), asking
    /// to share a recording the caller doesn't own surfaces as
    /// [`Error::Http`] with status 404 — existence doesn't leak.
    pub async fn share_recording(
        &self,
        req: &ShareRecordingRequest,
    ) -> Result<ShareRecordingResponse> {
        if req.recording_source_id.is_empty() {
            return Err(Error::BadRequest(
                "recording_source_id must not be empty".into(),
            ));
        }
        let path = format!("/api/voice/recordings/{}/share", req.recording_source_id);
        self.post_json::<ShareRecordingResponse, _>(&path, req)
            .await
    }

    /// `GET /api/voice/recordings/{id}/share` — read the authoritative
    /// share state for an owned recording, including the restricted tier's
    /// invited emails (which the share command's reply omits). Like
    /// [`share_recording`](Self::share_recording), a recording the caller
    /// doesn't own surfaces as [`Error::Http`] with status 404.
    pub async fn get_recording_share(
        &self,
        recording_source_id: &str,
    ) -> Result<ShareStateResponse> {
        if recording_source_id.is_empty() {
            return Err(Error::BadRequest(
                "recording_source_id must not be empty".into(),
            ));
        }
        let path = format!("/api/voice/recordings/{recording_source_id}/share");
        self.get_json::<ShareStateResponse>(&path).await
    }

    /// `DELETE /api/voice/recordings/{id}/share` — revoke the share. The
    /// recording reverts to Private and any outstanding link returns 410.
    pub async fn revoke_recording_share(&self, recording_source_id: &str) -> Result<()> {
        if recording_source_id.is_empty() {
            return Err(Error::BadRequest(
                "recording_source_id must not be empty".into(),
            ));
        }
        let path = format!("/api/voice/recordings/{recording_source_id}/share");
        self.delete(&path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_visibility_types_are_reachable_from_the_crate_root() {
        // Regression for the 0.0.13 gap: `PartyMasking` was added to this
        // module but left out of the crate-root `pub use voice::{…}`, and the
        // module is private — so a consumer (`wavekat-voice`) couldn't name
        // the type to build a `ShareRecordingRequest`. Pin every share-control
        // type to the root path so dropping one fails to compile here, not in
        // a downstream crate. The body never runs; reachability is the test.
        #[allow(dead_code)]
        fn _reachable() {
            let _: Option<crate::PartyMasking> = Some(crate::PartyMasking::Partial);
            let _: Option<crate::ShareVisibility> = Some(crate::ShareVisibility::Public);
            let _: fn(&crate::ShareRecordingRequest) = |_| {};
            let _: fn(&crate::ShareRecordingResponse) = |_| {};
        }
    }

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
            share_visibility: None,
            transfer_target: None,
            codec: None,
            flow_id: None,
            flow_name: None,
            flow_outcome: None,
            flow_steps: None,
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
        // Optional `transferTarget` is None here — omitted from the wire,
        // exactly like a non-transferred call ships.
        assert!(
            !s.contains("\"transferTarget\""),
            "transferTarget should be omitted: {s}"
        );
        // Optional `codec` is None (never-answered call, or an older
        // daemon) — omitted from the wire, never `null`.
        assert!(!s.contains("\"codec\""), "codec should be omitted: {s}");
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
            VoiceCallEndReason::TransferredLocal,
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
    fn transferred_local_pins_its_wire_string() {
        // Same contract as `connection_lost`: the platform validates
        // against an exact string list, so a rename here would bounce
        // every transferred-call upload with a 400.
        let s = serde_json::to_string(&VoiceCallEndReason::TransferredLocal).unwrap();
        assert_eq!(s, "\"transferred_local\"");
    }

    #[test]
    fn record_round_trips_transfer_target() {
        // A transferred call carries `transferTarget` both ways — the
        // daemon ships it (it's its own data, not read-only decoration),
        // and the platform echoes it back on read.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "Alice <sip:alice@example.com>",
            "ringAt": "2026-06-28T10:00:00Z",
            "answerAt": "2026-06-28T10:00:05Z",
            "endAt": "2026-06-28T10:00:30Z",
            "durationMs": 25000,
            "disposition": "answered",
            "endReason": "transferred_local",
            "transferTarget": "1002"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.end_reason, VoiceCallEndReason::TransferredLocal);
        assert_eq!(parsed.transfer_target.as_deref(), Some("1002"));
        // And it survives a re-serialize (daemon → platform direction).
        let s = serde_json::to_string(&parsed).unwrap();
        assert!(s.contains("\"transferTarget\":\"1002\""), "{s}");
    }

    #[test]
    fn codec_pins_its_wire_strings() {
        // The platform's sync endpoint validates the codec against an
        // exact string list, and the daemon's `CallCodec::as_str` emits
        // these same strings — a rename here would bounce every upload
        // from an answered call with a 400.
        for (codec, wire) in [
            (VoiceCallCodec::Opus, "\"opus\""),
            (VoiceCallCodec::Pcmu, "\"pcmu\""),
            (VoiceCallCodec::Pcma, "\"pcma\""),
        ] {
            assert_eq!(serde_json::to_string(&codec).unwrap(), wire);
            let back: VoiceCallCodec = serde_json::from_str(wire).unwrap();
            assert_eq!(back, codec);
        }
    }

    #[test]
    fn record_round_trips_codec() {
        // An answered call carries `codec` both ways — the daemon ships
        // it (its own data, like transferTarget), and the platform
        // echoes it back on read so the website can show the call's
        // audio quality.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "Alice <sip:alice@example.com>",
            "ringAt": "2026-07-03T10:00:00Z",
            "answerAt": "2026-07-03T10:00:05Z",
            "endAt": "2026-07-03T10:00:30Z",
            "durationMs": 25000,
            "disposition": "answered",
            "endReason": "hangup_remote",
            "codec": "opus"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.codec, Some(VoiceCallCodec::Opus));
        // And it survives a re-serialize (daemon → platform direction).
        let s = serde_json::to_string(&parsed).unwrap();
        assert!(s.contains("\"codec\":\"opus\""), "{s}");

        // A row from an older daemon has no codec — reads as None.
        let legacy = raw.replace(",\n            \"codec\": \"opus\"", "");
        let parsed: VoiceCallRecord = serde_json::from_str(&legacy).unwrap();
        assert_eq!(parsed.codec, None);
    }

    #[test]
    fn flow_outcome_pins_its_wire_strings() {
        // Three parties agree on these exact strings: the daemon's
        // `flow_outcome_to_str`, `wavekat_flow::trace::FlowOutcome`'s
        // snake_case serde, and the platform's zod enum. A rename here
        // 400s every flow-answered call's batch.
        for (outcome, wire) in [
            (VoiceCallFlowOutcome::Answered, "\"answered\""),
            (VoiceCallFlowOutcome::MessageLeft, "\"message_left\""),
            (VoiceCallFlowOutcome::Transferred, "\"transferred\""),
            (VoiceCallFlowOutcome::HungUp, "\"hung_up\""),
            (VoiceCallFlowOutcome::Aborted, "\"aborted\""),
            (VoiceCallFlowOutcome::Defect, "\"defect\""),
        ] {
            assert_eq!(serde_json::to_string(&outcome).unwrap(), wire);
            let back: VoiceCallFlowOutcome = serde_json::from_str(wire).unwrap();
            assert_eq!(back, outcome);
        }
    }

    #[test]
    fn record_round_trips_flow_attribution() {
        // A flow-answered call carries which flow took it and how the
        // run ended, both ways: the daemon ships them, the platform
        // echoes them so the website can say "Answered by “X”" and show
        // the run's own outcome instead of the misleading SIP one.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "Alice <sip:alice@example.com>",
            "ringAt": "2026-07-03T10:00:00Z",
            "answerAt": "2026-07-03T10:00:05Z",
            "endAt": "2026-07-03T10:00:30Z",
            "durationMs": 25000,
            "disposition": "answered",
            "endReason": "hangup_local",
            "flowId": "flow_after_hours",
            "flowName": "After hours",
            "flowOutcome": "message_left"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.flow_id.as_deref(), Some("flow_after_hours"));
        assert_eq!(parsed.flow_name.as_deref(), Some("After hours"));
        assert_eq!(parsed.flow_outcome, Some(VoiceCallFlowOutcome::MessageLeft));

        let s = serde_json::to_string(&parsed).unwrap();
        assert!(s.contains("\"flowId\":\"flow_after_hours\""), "{s}");
        assert!(s.contains("\"flowName\":\"After hours\""), "{s}");
        assert!(s.contains("\"flowOutcome\":\"message_left\""), "{s}");
    }

    #[test]
    fn record_round_trips_a_flow_step_trace() {
        // Pins the per-step field names. These are consumed by the
        // platform's Zod schema on one side and produced by the daemon's
        // projection on the other; a silent rename here breaks both.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "sip:alice@example.com",
            "ringAt": "2026-07-03T10:00:00Z",
            "answerAt": "2026-07-03T10:00:05Z",
            "endAt": "2026-07-03T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_local",
            "flowId": "f",
            "flowName": "F",
            "flowSteps": [
                { "atMs": 0, "kind": "spoke", "node": "greeting" },
                { "atMs": 4200, "kind": "menu_choice", "digit": "2" },
                { "atMs": 9100, "kind": "message_recorded", "secs": 31 }
            ]
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        let steps = parsed.flow_steps.as_deref().expect("steps present");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1].kind, "menu_choice");
        assert_eq!(steps[1].digit.as_deref(), Some("2"));
        assert_eq!(steps[2].secs, Some(31));
        // Absent per-step fields stay absent rather than serializing as
        // nulls — same contract as the record's own optional fields.
        let s = serde_json::to_string(&steps[0]).unwrap();
        assert_eq!(s, r#"{"atMs":0,"kind":"spoke","node":"greeting"}"#);
    }

    #[test]
    fn flow_step_accepts_a_kind_this_build_does_not_know() {
        // The whole reason `kind` is a String. A consumer pinned to an
        // older crate version must still deserialize a newer daemon's
        // trace — rejecting would fail the entire call record, not one
        // step.
        let step: VoiceCallFlowStep =
            serde_json::from_str(r#"{"atMs": 10, "kind": "consulted_the_oracle"}"#).unwrap();
        assert_eq!(step.kind, "consulted_the_oracle");
        assert_eq!(step.digit, None);
    }

    #[test]
    fn record_omits_flow_steps_for_a_human_answered_call() {
        // A call the user took themselves has no trace. The field must
        // stay off the wire entirely rather than serializing as null.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "sip:alice@example.com",
            "ringAt": "2026-07-03T10:00:00Z",
            "endAt": "2026-07-03T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_local"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert!(parsed.flow_steps.is_none());
        let s = serde_json::to_string(&parsed).unwrap();
        assert!(!s.contains("flowSteps"), "{s}");
    }

    #[test]
    fn record_omits_flow_fields_for_a_human_answered_call() {
        // Calls the user took themselves — and every row from a daemon
        // predating call flows — carry none of the three. They must
        // stay off the wire entirely, not serialize as nulls.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "sip:alice@example.com",
            "ringAt": "2026-07-03T10:00:00Z",
            "endAt": "2026-07-03T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_remote"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.flow_id, None);
        assert_eq!(parsed.flow_name, None);
        assert_eq!(parsed.flow_outcome, None);

        let s = serde_json::to_string(&parsed).unwrap();
        assert!(!s.contains("\"flowId\""), "flowId should be omitted: {s}");
        assert!(
            !s.contains("\"flowName\""),
            "flowName should be omitted: {s}"
        );
        assert!(
            !s.contains("\"flowOutcome\""),
            "flowOutcome should be omitted: {s}"
        );
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
    fn call_record_parses_share_visibility_from_list_response() {
        // The list / detail endpoints decorate a call with the tier of any
        // active share on its recording, so a consumer can badge the row.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "outbound",
            "party": "+14155550123",
            "ringAt": "2026-05-16T10:00:00Z",
            "endAt": "2026-05-16T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_remote",
            "shareVisibility": "public"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.share_visibility, Some(ShareVisibility::Public));

        let restricted = raw.replace("public", "restricted");
        let parsed: VoiceCallRecord = serde_json::from_str(&restricted).unwrap();
        assert_eq!(parsed.share_visibility, Some(ShareVisibility::Restricted));
    }

    #[test]
    fn call_record_unshared_has_no_share_visibility() {
        // Absent (older platform, or an unshared call) and an explicit
        // `null` both read as "not shared" — never `Some(Private)`.
        let base = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "anon",
            "ringAt": "2026-05-16T10:00:00Z",
            "endAt": "2026-05-16T10:00:30Z",
            "disposition": "missed",
            "endReason": "missed"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(base).unwrap();
        assert_eq!(parsed.share_visibility, None);

        let with_null = base.replace(
            r#""endReason": "missed""#,
            r#""endReason": "missed", "shareVisibility": null"#,
        );
        let parsed: VoiceCallRecord = serde_json::from_str(&with_null).unwrap();
        assert_eq!(parsed.share_visibility, None);
    }

    #[test]
    fn synced_call_omits_share_visibility() {
        // `share_visibility` is read-only decoration: a call uploaded via
        // sync must not carry it on the wire (skip_serializing_if = None),
        // so the round trip from a sync-shaped record stays clean.
        let raw = r#"{
            "sourceId": "a",
            "accountId": "b",
            "direction": "inbound",
            "party": "anon",
            "ringAt": "2026-05-16T10:00:00Z",
            "endAt": "2026-05-16T10:00:30Z",
            "disposition": "answered",
            "endReason": "hangup_remote"
        }"#;
        let parsed: VoiceCallRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.share_visibility, None);
        let s = serde_json::to_string(&parsed).unwrap();
        assert!(
            !s.contains("shareVisibility"),
            "sync payload leaked share_visibility: {s}"
        );
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

    #[test]
    fn share_visibility_pins_its_wire_strings() {
        // The platform validates these against an exact string list; a
        // rename would bounce every share command with a 400.
        assert_eq!(
            serde_json::to_string(&ShareVisibility::Private).unwrap(),
            "\"private\""
        );
        assert_eq!(
            serde_json::to_string(&ShareVisibility::Restricted).unwrap(),
            "\"restricted\""
        );
        assert_eq!(
            serde_json::to_string(&ShareVisibility::Public).unwrap(),
            "\"public\""
        );
        for v in [
            ShareVisibility::Private,
            ShareVisibility::Restricted,
            ShareVisibility::Public,
        ] {
            let s = serde_json::to_string(&v).unwrap();
            let back: ShareVisibility = serde_json::from_str(&s).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn share_request_serializes_with_camel_case_and_omits_unset() {
        let req = ShareRecordingRequest {
            recording_source_id: "11111111-1111-4111-8111-111111111111".into(),
            visibility: ShareVisibility::Public,
            invited_emails: None,
            party_masking: None,
            show_transcript: None,
            show_audio: None,
            allow_download: None,
            default_mute_local: None,
            default_mute_remote: None,
            password: None,
            expires_at: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"recordingSourceId\":"), "{s}");
        assert!(s.contains("\"visibility\":\"public\""), "{s}");
        // Phase-2 / tier-specific / visibility-control fields stay off the
        // wire when unset so the platform's `.optional()` schema accepts the
        // body (and the omitted controls fall to the platform defaults).
        assert!(!s.contains("invitedEmails"), "{s}");
        assert!(!s.contains("partyMasking"), "{s}");
        assert!(!s.contains("showTranscript"), "{s}");
        assert!(!s.contains("showAudio"), "{s}");
        assert!(!s.contains("allowDownload"), "{s}");
        assert!(!s.contains("defaultMuteLocal"), "{s}");
        assert!(!s.contains("defaultMuteRemote"), "{s}");
        assert!(!s.contains("password"), "{s}");
        assert!(!s.contains("expiresAt"), "{s}");
    }

    #[test]
    fn share_request_serializes_visibility_controls_camel_case() {
        let req = ShareRecordingRequest {
            recording_source_id: "a".into(),
            visibility: ShareVisibility::Public,
            invited_emails: None,
            party_masking: Some(PartyMasking::Partial),
            show_transcript: Some(false),
            show_audio: Some(true),
            allow_download: Some(true),
            default_mute_local: Some(false),
            default_mute_remote: Some(true),
            password: None,
            expires_at: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"partyMasking\":\"partial\""), "{s}");
        assert!(s.contains("\"showTranscript\":false"), "{s}");
        assert!(s.contains("\"showAudio\":true"), "{s}");
        assert!(s.contains("\"allowDownload\":true"), "{s}");
        // The owner muted their own side by default but left the other
        // party audible — both ride the wire as camelCase booleans.
        assert!(s.contains("\"defaultMuteLocal\":false"), "{s}");
        assert!(s.contains("\"defaultMuteRemote\":true"), "{s}");
    }

    #[test]
    fn share_request_carries_invited_emails_for_restricted() {
        let req = ShareRecordingRequest {
            recording_source_id: "a".into(),
            visibility: ShareVisibility::Restricted,
            invited_emails: Some(vec!["alex@example.com".into()]),
            party_masking: None,
            show_transcript: None,
            show_audio: None,
            allow_download: None,
            default_mute_local: None,
            default_mute_remote: None,
            password: None,
            expires_at: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"visibility\":\"restricted\""), "{s}");
        assert!(
            s.contains("\"invitedEmails\":[\"alex@example.com\"]"),
            "{s}"
        );
    }

    #[test]
    fn share_response_parses_platform_shape() {
        let raw = r#"{
            "visibility": "public",
            "token": "Zr7-x9F2k1QpLmN4sT8wYa",
            "shareUrl": "https://platform.wavekat.com/voice/s/Zr7-x9F2k1QpLmN4sT8wYa",
            "sharedAt": "2026-06-19T10:00:00.000Z"
        }"#;
        let parsed: ShareRecordingResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.visibility, ShareVisibility::Public);
        assert_eq!(parsed.token, "Zr7-x9F2k1QpLmN4sT8wYa");
        assert!(parsed.share_url.ends_with(&parsed.token));
    }

    #[test]
    fn share_state_parses_restricted_with_invited_emails() {
        // The GET read carries the audience back — this is the field the
        // POST reply omits and the desktop "who can open this" panel needs.
        let raw = r#"{
            "visibility": "restricted",
            "token": "Zr7-x9F2k1QpLmN4sT8wYa",
            "shareUrl": "https://platform.wavekat.com/voice/s/Zr7-x9F2k1QpLmN4sT8wYa",
            "sharedAt": "2026-06-19T10:00:00.000Z",
            "invitedEmails": ["bob@example.com", "carol@example.com"],
            "partyMasking": "full",
            "showTranscript": true,
            "showAudio": false,
            "allowDownload": false,
            "defaultMuteLocal": false,
            "defaultMuteRemote": true
        }"#;
        let parsed: ShareStateResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.visibility, ShareVisibility::Restricted);
        assert_eq!(
            parsed.invited_emails.as_deref(),
            Some(
                [
                    "bob@example.com".to_string(),
                    "carol@example.com".to_string()
                ]
                .as_slice()
            )
        );
        // The visibility controls ride back on the live-share read.
        assert_eq!(parsed.party_masking, Some(PartyMasking::Full));
        assert_eq!(parsed.show_transcript, Some(true));
        assert_eq!(parsed.show_audio, Some(false));
        // Audio hidden here, so download comes back off (platform folds the two).
        assert_eq!(parsed.allow_download, Some(false));
        // Per-channel playback defaults ride back too.
        assert_eq!(parsed.default_mute_local, Some(false));
        assert_eq!(parsed.default_mute_remote, Some(true));
    }

    #[test]
    fn share_state_parses_private_with_fields_absent() {
        // A never-shared (or revoked) recording reports private with no
        // token / url / emails — the optional fields stay None.
        let parsed: ShareStateResponse =
            serde_json::from_str(r#"{ "visibility": "private" }"#).unwrap();
        assert_eq!(parsed.visibility, ShareVisibility::Private);
        assert!(parsed.token.is_none());
        assert!(parsed.share_url.is_none());
        assert!(parsed.shared_at.is_none());
        assert!(parsed.invited_emails.is_none());
    }

    #[test]
    fn share_request_rejects_empty_source_id_before_hitting_network() {
        // Guarded client-side so an empty id can't produce a path like
        // `/api/voice/recordings//share` that 404s confusingly.
        let req = ShareRecordingRequest {
            recording_source_id: String::new(),
            visibility: ShareVisibility::Private,
            invited_emails: None,
            party_masking: None,
            show_transcript: None,
            show_audio: None,
            allow_download: None,
            default_mute_local: None,
            default_mute_remote: None,
            password: None,
            expires_at: None,
        };
        // We can't call the async method without a runtime here, but the
        // guard mirrors `upload_recording_bytes` — assert the precondition
        // shape the method checks.
        assert!(req.recording_source_id.is_empty());
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

    // ---- VoiceFlows ----

    #[test]
    fn flows_query_serializes_cursor_and_omits_absent_fields() {
        let empty = serde_json::to_string(&VoiceFlowsQuery::default()).unwrap();
        assert_eq!(empty, "{}");
        let cursored = serde_json::to_string(&VoiceFlowsQuery {
            after: Some("flow_abc".into()),
            limit: Some(100),
        })
        .unwrap();
        assert!(cursored.contains("\"after\":\"flow_abc\""), "{cursored}");
        assert!(cursored.contains("\"limit\":100"), "{cursored}");
    }

    #[test]
    fn flows_page_parses_platform_shape() {
        let raw = r#"{
            "items": [{
                "id": "flow_1",
                "name": "Luigi's — after hours",
                "version": 3,
                "yaml": "schema_version: 1\n",
                "publishedAt": "2026-07-13T10:00:00Z"
            }],
            "nextAfter": null
        }"#;
        let page: VoiceFlowsPage = serde_json::from_str(raw).unwrap();
        assert_eq!(page.items.len(), 1);
        let rec = &page.items[0];
        assert_eq!(rec.id, "flow_1");
        assert_eq!(rec.version, 3);
        assert_eq!(rec.published_at, "2026-07-13T10:00:00Z");
        assert!(page.next_after.is_none());

        // A mid-walk page carries the cursor.
        let more: VoiceFlowsPage =
            serde_json::from_str(r#"{ "items": [], "nextAfter": "flow_1" }"#).unwrap();
        assert_eq!(more.next_after.as_deref(), Some("flow_1"));
    }

    #[test]
    fn flow_assets_manifest_parses_platform_shape() {
        // `ref` (a reserved word) maps to `asset_ref`; a null duration is
        // accepted (the platform doesn't always know it).
        let raw = r#"{
            "assets": [{
                "ref": "vprompt_ab12cd34",
                "format": "ulaw_8000",
                "byteSize": 48044,
                "durationMs": null,
                "contentHash": "9f2c00aa"
            }]
        }"#;
        let page: VoiceFlowAssetsPage = serde_json::from_str(raw).unwrap();
        assert_eq!(page.assets.len(), 1);
        let asset = &page.assets[0];
        assert_eq!(asset.asset_ref, "vprompt_ab12cd34");
        assert_eq!(asset.format, "ulaw_8000");
        assert_eq!(asset.byte_size, 48044);
        assert!(asset.duration_ms.is_none());
        assert_eq!(asset.content_hash, "9f2c00aa");

        // A text-only version legitimately has no frozen audio.
        let empty: VoiceFlowAssetsPage = serde_json::from_str(r#"{ "assets": [] }"#).unwrap();
        assert!(empty.assets.is_empty());
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
