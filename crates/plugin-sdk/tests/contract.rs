use dotlog_plugin_sdk::{
    validate_manifest, Capability, CloseReason, OutboxOutcome, OutboxInput, PluginError,
    PluginManifest, SegmentClosedEvent, SegmentSnapshot,
};

fn manifest() -> PluginManifest {
    PluginManifest {
        id: "memos".into(),
        name: "Memos 发布".into(),
        version: "1.0.0".into(),
        api_version: dotlog_plugin_sdk::API_VERSION,
        capabilities: vec![Capability::Network, Capability::SecretStore],
        network_hosts: vec![],
        settings_schema: serde_json::json!({}),
        entry: "builtin:memos".into(),
    }
}

#[test]
fn manifest_roundtrip_uses_camel_case_and_snake_capabilities() {
    let raw = serde_json::to_string(&manifest()).unwrap();
    assert!(raw.contains("\"apiVersion\":1"));
    assert!(raw.contains("\"network\""));

    let parsed: PluginManifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.capabilities, vec![Capability::Network, Capability::SecretStore]);
    assert_eq!(parsed.entry, "builtin:memos");
}

#[test]
fn validate_manifest_accepts_well_formed() {
    validate_manifest(&manifest()).unwrap();
}

#[test]
fn validate_manifest_rejects_wrong_api_version() {
    let mut value = manifest();
    value.api_version = 99;
    assert!(matches!(
        validate_manifest(&value),
        Err(PluginError::Manifest(_))
    ));
}

#[test]
fn validate_manifest_rejects_bad_id_and_entry() {
    let mut value = manifest();
    value.id = "Memos!".into();
    assert!(matches!(
        validate_manifest(&value),
        Err(PluginError::Manifest(_))
    ));

    let mut value = manifest();
    value.entry = "/usr/bin/memos".into();
    assert!(matches!(
        validate_manifest(&value),
        Err(PluginError::Manifest(_))
    ));
}

/// 快照与事件可序列化（host → 线程 / 未来 sidecar 传输的前提）。
#[test]
fn segment_event_roundtrip() {
    let event = SegmentClosedEvent {
        segment: SegmentSnapshot {
            id: "0192".into(),
            start_at_ms: 1_000,
            end_at_ms: 4_000,
            duration_seconds: 3,
            title: "高数".into(),
            note: String::new(),
            tags: vec!["高数".into()],
            category_id: Some("cat".into()),
            category_name: Some("学习".into()),
            is_focus: true,
        },
        reason: CloseReason::Punch,
    };
    let raw = serde_json::to_string(&event).unwrap();
    let parsed: SegmentClosedEvent = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.reason, CloseReason::Punch);
    assert_eq!(parsed.segment.duration_seconds, 3);
}

/// 去重键与快照 payload 能走 serde 往返。
#[test]
fn outbox_input_payload_roundtrip() {
    let input = OutboxInput {
        dedupe_key: "memos:0192:publish".into(),
        event_type: "memos.publish".into(),
        payload: serde_json::json!({ "segmentId": "0192", "title": "高数" }),
    };
    let raw = serde_json::to_string(&input.payload).unwrap();
    assert!(raw.contains("segmentId"));
    // outcome 仅是标记类型，保证 Debug 可用（日志输出）。
    assert!(format!("{:?}", OutboxOutcome::Done { external_id: None }).contains("Done"));
}
