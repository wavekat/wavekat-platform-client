//! Call tasks — the desktop half of the wire (wavekat-voice docs/71).
//!
//! A *batch* is a list of calls to make, authored on the platform (web
//! UI, `wk tasks`, an agent). The desktop:
//!
//! 1. pulls open batches with their tasks ([`Client::active_task_batches`],
//!    cursor by id, the same shape as published flows),
//! 2. claims a task before dialing ([`Client::claim_task`]) so nobody
//!    else calls the same person,
//! 3. pushes what happened as immutable attempts through the usual sync
//!    contract (`client.sync::<VoiceTaskAttempts>(&items)`).
//!
//! The desktop never edits a batch or a task. The platform derives each
//! task's status and latest results from its attempts.
//!
//! Every field the platform may add later is optional here
//! (`#[serde(default)]`), per the crate's additive-only wire policy.

use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::sync::{HasSyncEnvelope, SyncEndpoint, SyncEnvelope};

/// One question the caller answers after a call, defined per batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskOutcomeField {
    /// Stable key the answer is stored under (`confirmed`).
    pub key: String,
    /// What the caller sees (`Confirmed?`).
    pub label: String,
    /// `yes_no` | `choice` | `text` | `number` | `date`, or a value a
    /// newer platform added. Render an unknown type as free text.
    #[serde(rename = "type")]
    pub field_type: String,
    /// The choices, for `choice` fields.
    #[serde(default)]
    pub options: Vec<String>,
    /// Must be answered before the task can be marked done.
    #[serde(default)]
    pub required: bool,
}

/// Progress counts for a batch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskCounts {
    pub total: u32,
    pub pending: u32,
    pub done: u32,
    pub skipped: u32,
    pub cancelled: u32,
    /// Pending tasks someone is calling right now.
    #[serde(default)]
    pub in_progress: u32,
}

/// One call to make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskRecord {
    /// Platform id (`task_…`).
    pub id: String,
    pub batch_id: String,
    /// Order within the batch, from the user's source list.
    pub position: u32,
    /// E.164 (`+64211234567`).
    pub phone: String,
    #[serde(default)]
    pub name: Option<String>,
    /// The task's purpose — its own, or the batch's when it has none.
    #[serde(default)]
    pub brief: Option<String>,
    /// Every other column from the source list, as text. Opaque.
    #[serde(default)]
    pub context: std::collections::BTreeMap<String, String>,
    /// RFC 3339 calling window, if the user set one.
    #[serde(default)]
    pub not_before: Option<String>,
    #[serde(default)]
    pub not_after: Option<String>,
    /// `pending` | `done` | `skipped` | `cancelled`, or a newer value.
    pub status: String,
    /// Latest answers to the batch's outcome fields.
    #[serde(default)]
    pub results: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub last_call_result: Option<String>,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub last_attempt_at: Option<String>,
    /// Someone is calling this task right now.
    #[serde(default)]
    pub in_progress: bool,
    #[serde(default)]
    pub claim_expires_at: Option<String>,
    /// The live claim is held by the device making this request.
    #[serde(default)]
    pub claimed_by_me: bool,
}

/// A batch with all its tasks, as the desktop pulls it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskBatchRecord {
    /// Platform id (`tb_…`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub outcome_fields: Vec<VoiceTaskOutcomeField>,
    #[serde(default)]
    pub cancelled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub counts: VoiceTaskCounts,
    #[serde(default)]
    pub tasks: Vec<VoiceTaskRecord>,
}

/// Query for `GET /api/voice/task-batches/active`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskBatchesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Batches per page, server-capped at 10 (each carries its tasks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One page of open batches. Walk `next_after` to `None` for the full
/// set; a local batch missing from a complete pull was cancelled or
/// deleted on the platform.
///
/// Compiled as a downstream crate, so it fails if these types stop being
/// re-exported from the crate root:
///
/// ```
/// use wavekat_platform_client::{TaskClaim, VoiceTaskAttemptRecord, VoiceTaskAttempts,
///     VoiceTaskBatchRecord, VoiceTaskBatchesPage, VoiceTaskBatchesQuery, VoiceTaskRecord};
///
/// let page: VoiceTaskBatchesPage = serde_json::from_str(r#"{"items":[],"nextAfter":null}"#).unwrap();
/// assert!(page.items.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskBatchesPage {
    pub items: Vec<VoiceTaskBatchRecord>,
    #[serde(default)]
    pub next_after: Option<String>,
}

/// The answer to a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskClaim {
    /// This device holds the task until `expires_at` (RFC 3339).
    Claimed { expires_at: String },
    /// Someone else is calling it; their claim runs until `expires_at`.
    Taken { expires_at: Option<String> },
    /// The task is no longer open (`done`, `skipped`, `cancelled`)
    /// since this device last pulled.
    NotOpen { status: Option<String> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_seconds: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimResponse {
    claim_expires_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimConflict {
    error: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    claim_expires_at: Option<String>,
}

/// Map a 409 body from `POST /tasks/{id}/claim` to a [`TaskClaim`].
/// `None` for a body that isn't the documented conflict shape.
fn claim_from_conflict(body: &str) -> Option<TaskClaim> {
    let c: ClaimConflict = serde_json::from_str(body).ok()?;
    match c.error.as_str() {
        "claimed" => Some(TaskClaim::Taken {
            expires_at: c.claim_expires_at,
        }),
        "not_open" => Some(TaskClaim::NotOpen { status: c.status }),
        _ => None,
    }
}

/// What one try at a task produced, pushed by the desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTaskAttemptRecord {
    /// The daemon's own id for this attempt — the sync idempotency key.
    pub source_id: String,
    pub task_id: String,
    /// `done` | `retry` | `skipped`.
    pub disposition: String,
    /// `answered` | `no_answer` | `busy` | `voicemail` | `wrong_number`
    /// | `failed`; `None` when no call was placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_result: Option<String>,
    /// The daemon's call id, linking the attempt to its recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_source_id: Option<String>,
    /// Answers keyed by outcome-field key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// RFC 3339.
    pub attempted_at: String,
    #[serde(flatten, default)]
    pub envelope: SyncEnvelope,
}

/// Unused: attempts are push-only, there is no list endpoint. Exists
/// because [`SyncEndpoint`] names a query type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceTaskAttemptsQuery {}

/// Marker for `POST /api/voice/task-attempts/sync`.
///
/// `client.sync::<VoiceTaskAttempts>(&items)`. Attempts for a task the
/// caller can no longer see come back counted in `skipped` rather than
/// failing the batch, so mark them synced like accepted rows.
pub struct VoiceTaskAttempts;

impl SyncEndpoint for VoiceTaskAttempts {
    const RESOURCE: &'static str = "task-attempts";
    type Record = VoiceTaskAttemptRecord;
    type Query = VoiceTaskAttemptsQuery;
}

impl HasSyncEnvelope for VoiceTaskAttemptRecord {
    fn envelope_mut(&mut self) -> &mut SyncEnvelope {
        &mut self.envelope
    }
}

impl Client {
    /// `GET /api/voice/task-batches/active` — one page of the caller's
    /// open batches, each with all its tasks.
    pub async fn active_task_batches(
        &self,
        query: &VoiceTaskBatchesQuery,
    ) -> Result<VoiceTaskBatchesPage> {
        self.get_json_query("/api/voice/task-batches/active", query)
            .await
    }

    /// `POST /api/voice/tasks/{id}/claim` — lease a task to this device
    /// before dialing. Re-claiming a task you hold extends the lease.
    /// `lease_seconds` `None` takes the platform default (10 minutes).
    ///
    /// A conflict is an answer, not an error: it comes back as
    /// [`TaskClaim::Taken`] or [`TaskClaim::NotOpen`].
    pub async fn claim_task(&self, task_id: &str, lease_seconds: Option<u32>) -> Result<TaskClaim> {
        let path = format!("/api/voice/tasks/{}/claim", urlencode(task_id));
        match self
            .post_json::<ClaimResponse, _>(&path, &ClaimRequest { lease_seconds })
            .await
        {
            Ok(r) => Ok(TaskClaim::Claimed {
                expires_at: r.claim_expires_at,
            }),
            Err(Error::Http {
                status: 409,
                ref body,
                ..
            }) if claim_from_conflict(body).is_some() => Ok(claim_from_conflict(body).unwrap()),
            Err(e) => Err(e),
        }
    }

    /// `POST /api/voice/tasks/{id}/release` — give a claimed task back
    /// without calling. A no-op for a task this device doesn't hold.
    pub async fn release_task(&self, task_id: &str) -> Result<()> {
        self.post_empty(&format!("/api/voice/tasks/{}/release", urlencode(task_id)))
            .await
    }
}

// Task ids are platform-minted (`task_<uuid>`), but they reach a URL
// path, so escape anything outside the unreserved set rather than trust
// the shape.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_page_decodes_the_platform_shape() {
        let page: VoiceTaskBatchesPage = serde_json::from_str(
            r#"{"items":[{"id":"tb_1","name":"Thursday","brief":"Confirm",
                "defaultCallingCode":"64",
                "outcomeFields":[{"key":"confirmed","label":"Confirmed?","type":"yes_no","required":true},
                                 {"key":"reason","label":"Why","type":"choice","options":["a","b"]}],
                "clientRef":null,"cancelledAt":null,
                "createdAt":"2026-10-12T10:00:00.000Z","updatedAt":"2026-10-12T10:00:00.000Z",
                "counts":{"total":1,"pending":1,"done":0,"skipped":0,"cancelled":0,"inProgress":0},
                "tasks":[{"id":"task_1","batchId":"tb_1","position":0,"phone":"+64211234567",
                  "name":"Ana","brief":"Confirm","context":{"appt":"Thu 10:30"},
                  "notBefore":null,"notAfter":null,"status":"pending","results":null,
                  "lastCallResult":null,"attemptCount":0,"lastAttemptAt":null,
                  "inProgress":false,"claimExpiresAt":null,"claimedByMe":false}]}],
              "nextAfter":null}"#,
        )
        .unwrap();
        let b = &page.items[0];
        assert_eq!(b.outcome_fields[0].field_type, "yes_no");
        assert!(b.outcome_fields[0].required);
        assert_eq!(b.outcome_fields[1].options, vec!["a", "b"]);
        assert_eq!(b.tasks[0].context["appt"], "Thu 10:30");
        assert_eq!(page.next_after, None);
    }

    #[test]
    fn task_tolerates_missing_optional_fields() {
        // An older or trimmed platform response: only the required keys.
        let t: VoiceTaskRecord = serde_json::from_str(
            r#"{"id":"task_1","batchId":"tb_1","position":3,"phone":"+64211234567","status":"pending"}"#,
        )
        .unwrap();
        assert_eq!(t.attempt_count, 0);
        assert!(t.context.is_empty());
        assert!(!t.claimed_by_me);
    }

    #[test]
    fn attempt_serialises_camel_case_and_omits_empty_fields() {
        let a = VoiceTaskAttemptRecord {
            source_id: "att_1".into(),
            task_id: "task_1".into(),
            disposition: "skipped".into(),
            call_result: None,
            call_source_id: None,
            results: None,
            note: None,
            attempted_at: "2026-10-12T10:00:00Z".into(),
            envelope: SyncEnvelope::default(),
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["sourceId"], "att_1");
        assert_eq!(v["attemptedAt"], "2026-10-12T10:00:00Z");
        assert!(v.get("callResult").is_none());
        assert!(v.get("results").is_none());
    }

    #[test]
    fn conflict_bodies_map_to_claim_answers() {
        assert_eq!(
            claim_from_conflict(r#"{"error":"claimed","claimExpiresAt":"2026-10-12T10:05:00Z"}"#),
            Some(TaskClaim::Taken {
                expires_at: Some("2026-10-12T10:05:00Z".into())
            })
        );
        assert_eq!(
            claim_from_conflict(r#"{"error":"not_open","status":"done"}"#),
            Some(TaskClaim::NotOpen {
                status: Some("done".into())
            })
        );
        assert_eq!(claim_from_conflict(r#"{"error":"something_else"}"#), None);
        assert_eq!(claim_from_conflict("not json"), None);
    }

    #[test]
    fn task_ids_are_escaped_in_paths() {
        assert_eq!(urlencode("task_abc-123"), "task_abc-123");
        assert_eq!(urlencode("a/b?c"), "a%2Fb%3Fc");
    }

    fn client_for(base: String) -> Client {
        Client::new(base, crate::token::Token::new("wk_test")).unwrap()
    }

    #[tokio::test]
    async fn claim_maps_success_and_both_conflicts() {
        use crate::client::tests::one_shot_server;
        let ok = client_for(one_shot_server(
            "200 OK",
            r#"{"taskId":"task_1","claimExpiresAt":"2026-10-12T10:10:00Z"}"#,
        ));
        assert_eq!(
            ok.claim_task("task_1", None).await.unwrap(),
            TaskClaim::Claimed {
                expires_at: "2026-10-12T10:10:00Z".into()
            }
        );
        let taken = client_for(one_shot_server(
            "409 Conflict",
            r#"{"error":"claimed","claimExpiresAt":"2026-10-12T10:05:00Z"}"#,
        ));
        assert!(matches!(
            taken.claim_task("task_1", Some(300)).await.unwrap(),
            TaskClaim::Taken { .. }
        ));
        let closed = client_for(one_shot_server(
            "409 Conflict",
            r#"{"error":"not_open","status":"cancelled"}"#,
        ));
        assert_eq!(
            closed.claim_task("task_1", None).await.unwrap(),
            TaskClaim::NotOpen {
                status: Some("cancelled".into())
            }
        );
    }

    #[tokio::test]
    async fn claim_passes_other_errors_through() {
        use crate::client::tests::one_shot_server;
        let c = client_for(one_shot_server("404 Not Found", r#"{"error":"not_found"}"#));
        assert!(matches!(
            c.claim_task("task_1", None).await,
            Err(Error::Http { status: 404, .. })
        ));
    }

    #[test]
    fn attempts_endpoint_targets_task_attempts() {
        assert_eq!(VoiceTaskAttempts::RESOURCE, "task-attempts");
    }
}
