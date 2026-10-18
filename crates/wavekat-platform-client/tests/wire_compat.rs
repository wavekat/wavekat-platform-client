//! Wire-compatibility tests: old consumer builds stay installed for
//! months, and the platform deploys first. These tests pin both
//! directions.
//!
//! * **Old payload, new code.** Every file under `tests/fixtures/<type>/`
//!   is a payload as an earlier release produced or consumed it, named for
//!   that release (`v0.0.22.json` = the shape at 0.0.22). The current
//!   types must still read it, with every field added since left `None`.
//! * **New payload, old code.** The same payloads with an extra field
//!   injected into every JSON object must still read, because the
//!   platform will add fields before every client knows them.
//!   (`deny_unknown_fields` must never be used on a wire type.)
//! * **No stray nulls.** Types a client sends must omit unset optional
//!   fields rather than send `null`, so an older platform never sees a
//!   key it did not expect.
//!
//! # Fixtures are append-only
//!
//! NEVER edit an existing fixture file. A fixture is a record of what a
//! past release put on the wire; changing it rewrites history and makes
//! the test lie. When a field is added to a type, add a new fixture
//! named for the release that ships it, plus assertions beside it here,
//! and leave the older files alone. If an old fixture stops parsing, the
//! change is breaking for installed clients: fix the code, not the file.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use wavekat_platform_client::*;

macro_rules! fixture {
    ($p:literal) => {
        include_str!(concat!("fixtures/", $p, ".json"))
    };
}

/// Add an unknown key to every object in the document, nested ones
/// included.
fn with_unknown_fields(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for child in m.values_mut() {
                with_unknown_fields(child);
            }
            m.insert(
                "fieldFromTheFuture".into(),
                json!({"nested": [1, "two", null]}),
            );
        }
        Value::Array(a) => a.iter_mut().for_each(with_unknown_fields),
        _ => {}
    }
}

fn assert_no_nulls(v: &Value, whole: &Value) {
    match v {
        Value::Null => panic!("null on the wire: {whole}"),
        Value::Object(m) => m.values().for_each(|c| assert_no_nulls(c, whole)),
        Value::Array(a) => a.iter().for_each(|c| assert_no_nulls(c, whole)),
        _ => {}
    }
}

/// Parse `raw`, and parse it again with unknown fields injected.
fn read<T: DeserializeOwned>(raw: &str) -> T {
    let mut v: Value = serde_json::from_str(raw).expect("fixture is valid JSON");
    let t: T = serde_json::from_value(v.clone()).expect("fixture must still parse");
    with_unknown_fields(&mut v);
    serde_json::from_value::<T>(v).expect("unknown extra fields must be ignored");
    t
}

/// For types a client sends: the fixture survives parse -> serialize
/// unchanged, which also proves unset optional fields are omitted
/// rather than written as `null`.
fn round_trips<T: DeserializeOwned + Serialize>(raw: &str) -> T {
    let t: T = read(raw);
    let want: Value = serde_json::from_str(raw).unwrap();
    let got = serde_json::to_value(&t).unwrap();
    assert_no_nulls(&got, &got);
    assert_eq!(got, want);
    t
}

// ---- sync records ---------------------------------------------------------

#[test]
fn voice_call_v0_0_4_has_none_of_the_later_fields() {
    let r: VoiceCallRecord = round_trips(fixture!("voice_call/v0.0.4"));
    assert_eq!(r.direction, VoiceCallDirection::Inbound);
    assert!(r.error.is_none() && r.share_visibility.is_none());
    assert!(r.transfer_target.is_none() && r.codec.is_none());
    assert!(r.flow_id.is_none() && r.flow_name.is_none());
    assert!(r.flow_outcome.is_none() && r.flow_steps.is_none());
    assert!(r.deleted_at.is_none());
    assert_eq!(r.envelope.schema_version, Some(1));
}

#[test]
fn voice_call_v0_0_22_predates_deleted_at() {
    let r: VoiceCallRecord = round_trips(fixture!("voice_call/v0.0.22"));
    assert_eq!(r.end_reason, VoiceCallEndReason::TransferredLocal);
    assert_eq!(r.codec, Some(VoiceCallCodec::Opus));
    assert_eq!(r.share_visibility, Some(ShareVisibility::Public));
    assert_eq!(r.flow_steps.as_ref().unwrap().len(), 3);
    assert!(r.deleted_at.is_none());
}

#[test]
fn voice_call_v0_0_36_reads_every_field() {
    let r: VoiceCallRecord = round_trips(fixture!("voice_call/v0.0.36"));
    assert_eq!(r.end_reason, VoiceCallEndReason::ConnectionLost);
    assert_eq!(r.flow_outcome, Some(VoiceCallFlowOutcome::MessageLeft));
    assert_eq!(r.deleted_at.as_deref(), Some("2026-06-01T00:00:00Z"));
    assert_eq!(r.envelope.extras, Some(json!({"x": 1})));
}

#[test]
fn voice_call_v0_0_36_predates_events_and_booking_details() {
    let r: VoiceCallRecord = round_trips(fixture!("voice_call/v0.0.36"));
    assert!(r.events.is_none());
    let step = &r.flow_steps.as_ref().unwrap()[0];
    assert!(step.count.is_none() && step.start.is_none());
}

#[test]
fn voice_call_v0_0_37_carries_events_and_booking_details() {
    let r: VoiceCallRecord = round_trips(fixture!("voice_call/v0.0.37"));
    let steps = r.flow_steps.as_ref().unwrap();
    assert_eq!(steps[0].count, Some(3));
    assert_eq!(steps[1].start.as_deref(), Some("2026-05-20T14:30:00+12:00"));
    let events = r.events.as_ref().unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[1].state.as_deref(), Some("ringing"));
    assert_eq!(events[3].digit.as_deref(), Some("5"));
    assert_eq!(events[4].status, Some(486));
}

#[test]
fn voice_call_event_accepts_a_kind_this_build_has_never_seen() {
    let raw = r#"{"atMs":5,"kind":"call_parked","slot":7}"#;
    let e: VoiceCallEvent = read(raw);
    assert_eq!(e.kind, "call_parked");
    assert!(e.state.is_none() && e.status.is_none() && e.digit.is_none());
}

#[test]
fn voice_call_flow_step_accepts_a_kind_this_build_has_never_seen() {
    let raw = r#"{"atMs":5,"kind":"some_future_node"}"#;
    let s: VoiceCallFlowStep = read(raw);
    assert_eq!(s.kind, "some_future_node");
    assert!(s.node.is_none() && s.digit.is_none() && s.open.is_none());
}

#[test]
fn voice_account_v0_0_32_reads_a_pre_tls_account() {
    let r: VoiceAccountRecord = round_trips(fixture!("voice_account/v0.0.32"));
    assert_eq!(r.transport, VoiceTransport::Udp);
    assert_eq!(r.keepalive_secs, Some(25));
    assert!(r.deleted_at.is_none());
}

#[test]
fn voice_account_v0_0_36_minimal_optionals() {
    let r: VoiceAccountRecord = round_trips(fixture!("voice_account/v0.0.36"));
    assert_eq!(r.transport, VoiceTransport::Tls);
    assert!(r.auth_username.is_none() && r.server.is_none() && r.port.is_none());
    assert!(r.keepalive_secs.is_none());
}

#[test]
fn voice_account_unknown_transport_does_not_fail_the_record() {
    let mut v: Value = serde_json::from_str(fixture!("voice_account/v0.0.36")).unwrap();
    v["transport"] = json!("quic");
    let r: VoiceAccountRecord = serde_json::from_value(v).unwrap();
    assert_eq!(r.transport, VoiceTransport::Unknown);
}

#[test]
fn voice_recording_v0_0_5() {
    let r: VoiceRecordingRecord = round_trips(fixture!("voice_recording/v0.0.5"));
    assert_eq!((r.sample_rate, r.channels), (16000, 2));
}

#[test]
fn voice_transcript_v0_0_5() {
    let r: VoiceTranscriptRecord = round_trips(fixture!("voice_transcript/v0.0.5"));
    assert_eq!(r.channel, VoiceTranscriptChannel::Remote);
}

#[test]
fn list_page_v0_0_4_reads_calls_and_cursor() {
    let p: Page<VoiceCallRecord> = round_trips(fixture!("page_calls/v0.0.4"));
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.next_before.as_deref(), Some("2026-05-16T10:01:00Z"));
    // The last page omits the cursor.
    let p: Page<VoiceCallRecord> = read(r#"{"items":[]}"#);
    assert!(p.next_before.is_none());
}

#[test]
fn sync_response_v0_0_4() {
    let r: SyncResponse = read(fixture!("sync_response/v0.0.4"));
    assert_eq!((r.accepted, r.skipped), (3, 1));
}

#[test]
fn recordings_sync_response_reads_items() {
    let r: VoiceRecordingsSyncResponse = read(
        r#"{"accepted":1,"skipped":0,"items":[{"sourceId":"r1","r2Key":"k","bytesUploaded":false}]}"#,
    );
    assert_eq!(r.items[0].r2_key, "k");
}

#[test]
fn queries_omit_unset_fields() {
    for v in [
        serde_json::to_value(VoiceCallsQuery::default()).unwrap(),
        serde_json::to_value(VoiceRecordingsQuery::default()).unwrap(),
        serde_json::to_value(VoiceFlowsQuery::default()).unwrap(),
    ] {
        assert_eq!(v, json!({}));
    }
    let q = VoiceAccountsQuery {
        include_deleted: None,
        transports: None,
    };
    assert_eq!(serde_json::to_value(q).unwrap(), json!({}));
}

// ---- install heartbeat and usage events -----------------------------------

#[test]
fn heartbeat_request_v0_0_24_predates_distribution_and_fleet() {
    let r: InstallHeartbeatRequest = round_trips(fixture!("install_heartbeat_request/v0.0.24"));
    assert!(r.distribution.is_none());
    assert_eq!(r.fleet, InstallHeartbeatFleet::default());
}

#[test]
fn heartbeat_request_v0_0_29_predates_fleet() {
    let r: InstallHeartbeatRequest = round_trips(fixture!("install_heartbeat_request/v0.0.29"));
    assert_eq!(r.distribution.as_deref(), Some("flatpak"));
    assert_eq!(r.fleet, InstallHeartbeatFleet::default());
}

#[test]
fn heartbeat_request_v0_0_34_predates_install_metadata() {
    let r: InstallHeartbeatRequest = round_trips(fixture!("install_heartbeat_request/v0.0.34"));
    assert_eq!(r.fleet.flow_armed, Some(true));
    assert!(r.fleet.timezone.is_none() && r.fleet.app_language.is_none());
    assert!(r.fleet.sip_accounts_total.is_none() && r.fleet.signed_in.is_none());
    assert!(r.fleet.cpu_cores.is_none() && r.fleet.memory_gb.is_none());
}

#[test]
fn heartbeat_request_v0_0_36_reads_install_metadata() {
    let r: InstallHeartbeatRequest = round_trips(fixture!("install_heartbeat_request/v0.0.36"));
    assert_eq!(r.fleet.memory_gb, Some(32));
    assert_eq!(r.fleet.signed_in, Some(false));
}

#[test]
fn heartbeat_response_v0_0_24_predates_distribution() {
    let r: InstallHeartbeatResponse = read(fixture!("install_heartbeat_response/v0.0.24"));
    assert!(r.distribution.is_none());
}

#[test]
fn heartbeat_response_v0_0_36_tolerates_null_optionals() {
    let r: InstallHeartbeatResponse = read(fixture!("install_heartbeat_response/v0.0.36"));
    assert_eq!(r.distribution.as_deref(), Some("dmg"));
    assert!(r.os_version.is_none() && r.arch.is_none() && r.locale.is_none());
}

#[test]
fn usage_events_v0_0_31() {
    let r: UsageEventsRequest = round_trips(fixture!("usage_events_request/v0.0.31"));
    assert!(r.events[0].detail.is_none());
    let r: UsageEventsResponse = read(r#"{"accepted":2}"#);
    assert_eq!(r.accepted, 2);
}

// ---- account and identity -------------------------------------------------

#[test]
fn me_v0_0_1() {
    let m: Me = read(fixture!("me/v0.0.1"));
    assert_eq!(m.login, "eason");
    assert!(m.email.is_none());
    // Optional profile fields may be missing altogether.
    let m: Me = read(r#"{"id":"u1","login":"e","role":"user"}"#);
    assert!(m.name.is_none());
}

#[test]
fn deletion_preview_v0_0_28() {
    let p: DeletionPreview = round_trips(fixture!("deletion_preview/v0.0.28"));
    assert_eq!((p.accounts, p.label_sets), (2, 0));
}

// ---- flows ----------------------------------------------------------------

#[test]
fn published_flows_page_v0_0_19() {
    let p: VoiceFlowsPage = read(fixture!("voice_flows_page/v0.0.19"));
    assert_eq!(p.items[0].version, 3);
    assert!(p.next_after.is_none());
    // The cursor may be absent rather than null.
    let p: VoiceFlowsPage = read(r#"{"items":[]}"#);
    assert!(p.next_after.is_none());
}

#[test]
fn system_flows_page_v0_0_26() {
    let p: VoiceSystemFlowsPage = read(fixture!("voice_system_flows_page/v0.0.26"));
    assert_eq!(p.flows[0].system_tags, ["booking"]);
    // Optional pieces missing entirely.
    let p: VoiceSystemFlowsPage = read(
        r#"{"flows":[{"id":"s","name":"n","language":"en","version":1,"yaml":"","access":"public"}]}"#,
    );
    assert!(p.flows[0].description.is_empty() && p.flows[0].published_at.is_none());
    assert!(p.flows[0].system_tags.is_empty());
}

#[test]
fn flow_assets_page_reads_optional_duration() {
    let p: VoiceFlowAssetsPage =
        read(r#"{"assets":[{"ref":"a","format":"ulaw_8000","byteSize":10,"contentHash":"h"}]}"#);
    assert!(p.assets[0].duration_ms.is_none());
}

// ---- booking --------------------------------------------------------------

#[test]
fn booking_slots_response_v0_0_24() {
    let r: BookingSlotsResponse = read(fixture!("booking_slots_response/v0.0.24"));
    assert!(r.status.is_none() && r.reason.is_none());
    // An `unavailable` answer carries no slots or timezone.
    let r: BookingSlotsResponse = read(r#"{"status":"unavailable","reason":"timeout"}"#);
    assert!(r.slots.is_empty() && r.timezone.is_empty());
}

#[test]
fn booking_book_response_v0_0_24() {
    let r: BookingBookResponse = read(fixture!("booking_book_response/v0.0.24"));
    assert_eq!(r.status, "booked");
    assert!(r.reason.is_none());
}

#[test]
fn booking_requests_omit_unset_fields() {
    let req = BookingBookRequest {
        source_id: "s".into(),
        start: "t".into(),
        duration_mins: 30,
        timezone: "UTC".into(),
        caller_number: String::new(),
        caller_name: None,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_no_nulls(&v, &v);
    let _: BookingBookRequest = read(&v.to_string());
    // A pre-`caller_name` platform payload lacks the optional fields.
    let _: BookingBookRequest =
        read(r#"{"source_id":"s","start":"t","duration_mins":30,"timezone":"UTC"}"#);
}

// ---- sharing --------------------------------------------------------------

#[test]
fn share_state_v0_0_12_predates_default_mute() {
    let r: ShareStateResponse = round_trips(fixture!("share_state/v0.0.12"));
    assert!(r.default_mute_local.is_none() && r.default_mute_remote.is_none());
    assert_eq!(r.party_masking, Some(PartyMasking::Partial));
}

#[test]
fn share_state_v0_0_36() {
    let r: ShareStateResponse = round_trips(fixture!("share_state/v0.0.36"));
    assert_eq!(r.default_mute_local, Some(true));
    assert!(r.invited_emails.is_none());
}

#[test]
fn share_state_for_an_unshared_call_is_just_a_visibility() {
    let r: ShareStateResponse = read(r#"{"visibility":"private"}"#);
    assert!(r.token.is_none() && r.share_url.is_none());
}

#[test]
fn share_recording_response_v0_0_12() {
    let r: ShareRecordingResponse = round_trips(fixture!("share_recording_response/v0.0.12"));
    assert!(r.party_masking.is_none() && r.default_mute_local.is_none());
}

#[test]
fn share_recording_request_omits_unset_fields() {
    let req: ShareRecordingRequest = read(r#"{"recordingSourceId":"r1","visibility":"public"}"#);
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v, json!({"recordingSourceId":"r1","visibility":"public"}));
}

// ---- prompts --------------------------------------------------------------

#[test]
fn prompt_generated_v0_0_36() {
    let r: VoicePromptGenerated = read(fixture!("voice_prompt_generated/v0.0.36"));
    assert_eq!(r.format, VoicePromptFormat::Ulaw8000);
    assert!(r.rendered_text.is_none() && r.expires_at.is_none());
    // The optional quota/allowance fields can be missing altogether.
    let r: VoicePromptGenerated = read(
        r#"{"id":"p","bytesUrl":"u","voiceId":"v","format":"mp3","preset":"digits","sourceText":"1","charCount":1,"saved":false}"#,
    );
    assert!(r.quota.is_none() && !r.ephemeral);
}

#[test]
fn prompt_generate_request_omits_default_ephemeral() {
    let v = serde_json::to_value(VoicePromptGenerateRequest::new("hi", "v1")).unwrap();
    assert!(v.get("ephemeral").is_none());
    assert_no_nulls(&v, &v);
}
