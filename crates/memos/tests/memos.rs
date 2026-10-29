//! Memos 插件测试：探测、入队规则、发布分类、幂等与配置缺失（§7）。

use std::sync::Mutex;

use dotlog_memos::endpoints::{self, ProbeOutcome};
use dotlog_memos::{DEFAULT_TEMPLATE, EVENT_TYPE, MemosPlugin, PLUGIN_ID, PublishHandler, render_content};
use dotlog_plugin_sdk::{
    CloseReason, HttpClient, HttpRequest, HttpResponse, IntegrationLinks, LinkRecord, OutboxEnv,
    OutboxHandler, OutboxInput, OutboxOutcome, OutboxSink, Plugin, PluginCtx, PluginError,
    PluginLogger, PluginSettings, SecretStore, SegmentClosedEvent, SegmentSnapshot,
};
use serde_json::json;

// ---------- 测试替身 ----------

enum FakeResponse {
    Http(u16, &'static str),
    Network,
    Timeout,
}

struct Rule {
    method: &'static str,
    url_suffix: &'static str,
    response: FakeResponse,
}

struct Captured {
    method: String,
    url: String,
    authorization: Option<String>,
    body: Option<String>,
}

#[derive(Default)]
struct MockHttp {
    rules: Mutex<Vec<Rule>>,
    captured: Mutex<Vec<Captured>>,
}

impl MockHttp {
    fn with_rule(method: &'static str, url_suffix: &'static str, response: FakeResponse) -> Self {
        let http = Self::default();
        http.rules.lock().unwrap().push(Rule { method, url_suffix, response });
        http
    }
}

impl HttpClient for MockHttp {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        self.captured.lock().unwrap().push(Captured {
            method: request.method.to_string(),
            url: request.url.to_string(),
            authorization: request
                .headers
                .iter()
                .find(|(name, _)| name == "Authorization")
                .map(|(_, value)| value.clone()),
            body: request
                .body
                .as_ref()
                .map(|body| String::from_utf8_lossy(body).into_owned()),
        });
        let rules = self.rules.lock().unwrap();
        let matched = rules
            .iter()
            .find(|rule| request.method == rule.method && request.url.ends_with(rule.url_suffix));
        match matched {
            Some(Rule { response: FakeResponse::Http(status, body), .. }) => Ok(HttpResponse {
                status: *status,
                body: body.as_bytes().to_vec(),
            }),
            Some(Rule { response: FakeResponse::Network, .. }) => {
                Err(PluginError::Network("connection refused".into()))
            }
            Some(Rule { response: FakeResponse::Timeout, .. }) => Err(PluginError::Timeout),
            None => Ok(HttpResponse { status: 404, body: Vec::new() }),
        }
    }
}

struct MockSettings {
    entries: Vec<(&'static str, serde_json::Value)>,
}

impl PluginSettings for MockSettings {
    fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.entries
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    }
}

struct MockSecrets {
    token: Option<&'static str>,
}

impl SecretStore for MockSecrets {
    fn get_secret(&self, _key: &str) -> Result<Option<String>, PluginError> {
        Ok(self.token.map(str::to_string))
    }

    fn set_secret(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
        Ok(())
    }

    fn delete_secret(&self, _key: &str) -> Result<(), PluginError> {
        Ok(())
    }
}

#[derive(Default)]
struct MockLinks {
    published: Option<(&'static str, &'static str)>,
}

impl IntegrationLinks for MockLinks {
    fn get(&self, segment_id: &str, provider: &str) -> Result<Option<LinkRecord>, PluginError> {
        Ok(self
            .published
            .filter(|(segment, _)| *segment == segment_id && provider == PLUGIN_ID)
            .map(|(segment, external)| LinkRecord {
                segment_id: segment.to_string(),
                provider: PLUGIN_ID.into(),
                external_id: Some(external.to_string()),
                state: "published".into(),
                last_synced_at_ms: Some(0),
                last_error: None,
            }))
    }

    fn mark_published(
        &self,
        _segment_id: &str,
        _provider: &str,
        _external_id: &str,
        _now_ms: i64,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn mark_failed(
        &self,
        _segment_id: &str,
        _provider: &str,
        _error: &str,
        _now_ms: i64,
    ) -> Result<(), PluginError> {
        Ok(())
    }
}

#[derive(Default)]
struct MockOutbox {
    events: Mutex<Vec<OutboxInput>>,
}

impl OutboxSink for MockOutbox {
    fn enqueue(&self, event: OutboxInput) -> Result<(), PluginError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

struct MockLogger;

impl PluginLogger for MockLogger {
    fn info(&self, _message: &str) {}
    fn warn(&self, _message: &str) {}
    fn error(&self, _message: &str) {}
}

static LOGGER: MockLogger = MockLogger;
static EMPTY_LINKS: MockLinks = MockLinks { published: None };

// ---------- 构造辅助 ----------

fn base_settings() -> Vec<(&'static str, serde_json::Value)> {
    vec![("instanceUrl", json!("https://memos.example.com"))]
}

fn settings(extra: Vec<(&'static str, serde_json::Value)>) -> MockSettings {
    let mut entries = base_settings();
    entries.extend(extra);
    MockSettings { entries }
}

fn snapshot() -> SegmentSnapshot {
    SegmentSnapshot {
        id: "0192-seg".into(),
        start_at_ms: local_ms(9, 0),
        end_at_ms: local_ms(10, 12),
        duration_seconds: 4_320,
        title: "高数 880 题 21–35".into(),
        note: "第二节卡了很久".into(),
        tags: vec!["学习".into(), "高数".into()],
        category_id: None,
        category_name: Some("学习".into()),
        is_focus: true,
    }
}

/// 本地时区固定时刻，保证渲染断言与时区无关。
fn local_ms(hour: u32, minute: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(2026, 10, 29, hour, minute, 0)
        .single()
        .unwrap()
        .timestamp_millis()
}

fn event(segment: SegmentSnapshot) -> SegmentClosedEvent {
    SegmentClosedEvent {
        segment,
        reason: CloseReason::Punch,
    }
}

fn plugin_ctx<'a>(settings: &'a MockSettings, outbox: &'a MockOutbox) -> PluginCtx<'a> {
    PluginCtx {
        plugin_id: PLUGIN_ID,
        segments: None,
        tags: None,
        http: None,
        secrets: None,
        outbox,
        settings,
        links: &EMPTY_LINKS,
        log: &LOGGER,
        now_ms: 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn handler_env<'a>(
    http: &'a MockHttp,
    secrets: &'a MockSecrets,
    settings: &'a MockSettings,
    links: &'a MockLinks,
) -> OutboxEnv<'a> {
    OutboxEnv {
        http: Some(http),
        secrets: Some(secrets),
        settings,
        links,
        log: &LOGGER,
        now_ms: 0,
    }
}

fn publish_payload() -> serde_json::Value {
    json!({ "segmentId": "seg-1", "title": "标题", "content": "正文内容" })
}

fn assert_done(outcome: OutboxOutcome, expected: Option<&str>) {
    match outcome {
        OutboxOutcome::Done { external_id } => assert_eq!(external_id.as_deref(), expected),
        other => panic!("期望 Done({expected:?})，实际 {other:?}"),
    }
}

fn assert_fail_contains(outcome: OutboxOutcome, needle: &str) {
    match outcome {
        OutboxOutcome::Fail { error } => assert!(error.contains(needle), "错误信息缺 `{needle}`：{error}"),
        other => panic!("期望 Fail（含 {needle}），实际 {other:?}"),
    }
}

fn assert_retry(outcome: OutboxOutcome) {
    assert!(matches!(outcome, OutboxOutcome::RetryLater { .. }), "期望 RetryLater，实际 {outcome:?}");
}

// ---------- 探测（§7.2） ----------

#[test]
fn probe_uses_status_endpoint() {
    let http = MockHttp::with_rule("GET", "/api/v1/status", FakeResponse::Http(200, "{}"));
    let outcome = endpoints::probe(&http, "https://memos.example.com/", None);
    assert_eq!(outcome, Ok(ProbeOutcome::Ok { api_version: "v1" }));
    let captured = http.captured.lock().unwrap();
    // 探测按候选顺序尝试，status 在第三位命中
    assert!(captured.iter().any(|c| c.url == "https://memos.example.com/api/v1/status"));
}

#[test]
fn probe_falls_back_to_profile() {
    let http = MockHttp::with_rule("GET", "/api/v1/workspace/profile", FakeResponse::Http(200, "{}"));
    let outcome = endpoints::probe(&http, "https://memos.example.com", None);
    assert_eq!(outcome, Ok(ProbeOutcome::Ok { api_version: "v1" }));
}

#[test]
fn probe_reports_unreachable() {
    let http = MockHttp::default();
    let outcome = endpoints::probe(&http, "https://memos.example.com", None);
    assert!(outcome.is_err());
    assert!(outcome.unwrap_err().contains("未找到匹配"));
}

#[test]
fn probe_detects_auth_issue() {
    let http = MockHttp::with_rule("GET", "/api/v1/memos", FakeResponse::Http(401, "{}"));
    let outcome = endpoints::probe(&http, "https://memos.example.com", Some("bad-token"));
    assert_eq!(outcome, Ok(ProbeOutcome::AuthIssue { api_version: "v1" }));
}

#[test]
fn probe_rejects_invalid_base_url() {
    let http = MockHttp::default();
    let outcome = endpoints::probe(&http, "ftp://memos.example.com", None);
    assert!(outcome.unwrap_err().contains("http(s)"));
}

// ---------- 段关闭 → 入队 ----------

#[test]
fn on_close_enqueues_publish_event() {
    let settings = settings(vec![("autoPublish", json!("on_close"))]);
    let outbox = MockOutbox::default();
    MemosPlugin
        .on_segment_closed(&plugin_ctx(&settings, &outbox), &event(snapshot()))
        .unwrap();

    let events = outbox.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].dedupe_key, "memos:0192-seg:publish");
    assert_eq!(events[0].event_type, EVENT_TYPE);
    let content = events[0].payload["content"].as_str().unwrap();
    assert!(content.contains("**09:00–10:12（1h12m）** 高数 880 题 21–35"));
    assert!(content.contains("#学习 #高数"));
    // 默认只发标题，不发备注（§7.4）。
    assert!(!content.contains("卡了很久"));
    assert!(content.ends_with("<!-- dotlog:segment=0192-seg -->"));
}

#[test]
fn auto_publish_off_by_default() {
    let settings = settings(Vec::new());
    let outbox = MockOutbox::default();
    MemosPlugin
        .on_segment_closed(&plugin_ctx(&settings, &outbox), &event(snapshot()))
        .unwrap();
    assert!(outbox.events.lock().unwrap().is_empty());
}

#[test]
fn tag_rule_filters_by_tag() {
    let settings = settings(vec![
        ("autoPublish", json!("tag_rule")),
        ("tagRules", json!("阅读, 学习")),
    ]);
    let outbox = MockOutbox::default();
    MemosPlugin
        .on_segment_closed(&plugin_ctx(&settings, &outbox), &event(snapshot()))
        .unwrap();
    assert_eq!(outbox.events.lock().unwrap().len(), 1);

    let mut other = snapshot();
    other.tags = vec!["运动".into()];
    MemosPlugin
        .on_segment_closed(&plugin_ctx(&settings, &outbox), &event(other))
        .unwrap();
    assert_eq!(outbox.events.lock().unwrap().len(), 1);
}

#[test]
fn missing_instance_url_skips_enqueue() {
    let settings = MockSettings {
        entries: vec![("autoPublish", json!("on_close"))],
    };
    let outbox = MockOutbox::default();
    MemosPlugin
        .on_segment_closed(&plugin_ctx(&settings, &outbox), &event(snapshot()))
        .unwrap();
    assert!(outbox.events.lock().unwrap().is_empty());
}

// ---------- 发布内容渲染（§7.3） ----------

#[test]
fn render_content_supports_note_and_template() {
    let content = render_content(&snapshot(), DEFAULT_TEMPLATE, true);
    assert!(content.contains("第二节卡了很久"));

    let template = "**{title}**（{duration}）\n{note}\n{tags}\n<!-- dotlog:segment={segmentId} -->";
    let custom = render_content(&snapshot(), template, false);
    assert!(custom.starts_with("**高数 880 题 21–35**（1h12m）"));
    assert!(custom.ends_with("<!-- dotlog:segment=0192-seg -->"));
}

// ---------- 发布 handler ----------

#[test]
fn publish_success_maps_proto_visibility() {
    let http = MockHttp::with_rule(
        "POST",
        "/api/v1/memos",
        FakeResponse::Http(200, r#"{"name":"memos/123","content":"正文内容"}"#),
    );
    let secrets = MockSecrets { token: Some("tok") };
    let settings = settings(vec![("visibility", json!("public"))]);
    let links = MockLinks::default();
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));

    assert_done(outcome, Some("memos/123"));
    let captured = http.captured.lock().unwrap();
    assert_eq!(captured[0].method, "POST");
    assert_eq!(captured[0].url, "https://memos.example.com/api/v1/memos");
    assert_eq!(captured[0].authorization.as_deref(), Some("Bearer tok"));
    let body = captured[0].body.as_deref().unwrap();
    assert!(body.contains(r#""visibility":"PUBLIC""#));
    assert!(body.contains("正文内容"));
}

#[test]
fn publish_is_idempotent_when_link_published() {
    let http = MockHttp::default();
    let secrets = MockSecrets { token: Some("tok") };
    let settings = settings(Vec::new());
    let links = MockLinks {
        published: Some(("seg-1", "memos/99")),
    };
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));

    assert_done(outcome, Some("memos/99"));
    assert!(http.captured.lock().unwrap().is_empty());
}

#[test]
fn publish_4xx_fails_without_retry() {
    let http = MockHttp::with_rule("POST", "/api/v1/memos", FakeResponse::Http(401, r#"{"message":"unauthorized"}"#));
    let secrets = MockSecrets { token: Some("tok") };
    let settings = settings(Vec::new());
    let links = MockLinks::default();
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_fail_contains(outcome, "401");
}

#[test]
fn publish_5xx_and_network_errors_retry() {
    let secrets = MockSecrets { token: Some("tok") };
    let settings = settings(Vec::new());
    let links = MockLinks::default();

    let http = MockHttp::with_rule("POST", "/api/v1/memos", FakeResponse::Http(502, ""));
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_retry(outcome);

    let http = MockHttp::with_rule("POST", "/api/v1/memos", FakeResponse::Network);
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_retry(outcome);

    let http = MockHttp::with_rule("POST", "/api/v1/memos", FakeResponse::Timeout);
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_retry(outcome);
}

#[test]
fn publish_missing_token_fails() {
    let http = MockHttp::default();
    let secrets = MockSecrets { token: None };
    let settings = settings(Vec::new());
    let links = MockLinks::default();
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_fail_contains(outcome, "令牌");
}

#[test]
fn publish_missing_instance_fails() {
    let http = MockHttp::default();
    let secrets = MockSecrets { token: Some("tok") };
    let settings = MockSettings { entries: Vec::new() };
    let links = MockLinks::default();
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_fail_contains(outcome, "instanceUrl");
}

#[test]
fn publish_unsupported_api_version_fails() {
    let http = MockHttp::default();
    let secrets = MockSecrets { token: Some("tok") };
    let settings = settings(vec![("apiVersion", json!("v9"))]);
    let links = MockLinks::default();
    let outcome = PublishHandler.handle(&publish_payload(), &handler_env(&http, &secrets, &settings, &links));
    assert_fail_contains(outcome, "不支持的 Memos API 版本");
}
