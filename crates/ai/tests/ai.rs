//! AI 提供方测试：隐私裁剪、端点请求形状、建议解析与清洗（§8）。

use std::sync::Mutex;

use dotlog_ai::{AiError, AiProvider, ClassifyInput, OpenAiCompatibleProvider, PrivacyMode};
use dotlog_plugin_sdk::{HttpClient, HttpRequest, HttpResponse, PluginError, SegmentSnapshot};
use serde_json::json;

// ---------- 测试替身 ----------

enum FakeResponse {
    Http(u16, String),
    Network,
    Timeout,
}

struct Rule {
    url_suffix: &'static str,
    response: FakeResponse,
}

struct Captured {
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
    fn with_rule(url_suffix: &'static str, response: FakeResponse) -> Self {
        let http = Self::default();
        http.rules.lock().unwrap().push(Rule {
            url_suffix,
            response,
        });
        http
    }
}

impl HttpClient for MockHttp {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        self.captured.lock().unwrap().push(Captured {
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
            .find(|rule| request.url.ends_with(rule.url_suffix));
        match matched {
            Some(Rule {
                response: FakeResponse::Http(status, body),
                ..
            }) => Ok(HttpResponse {
                status: *status,
                body: body.as_bytes().to_vec(),
            }),
            Some(Rule {
                response: FakeResponse::Network,
                ..
            }) => Err(PluginError::Network("connection refused".into())),
            Some(Rule {
                response: FakeResponse::Timeout,
                ..
            }) => Err(PluginError::Timeout),
            None => Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            }),
        }
    }
}

// ---------- 构造辅助 ----------

fn snapshot() -> SegmentSnapshot {
    SegmentSnapshot {
        id: "seg-1".into(),
        start_at_ms: local_ms(9, 0),
        end_at_ms: local_ms(10, 12),
        duration_seconds: 4_320,
        title: "看文献".into(),
        note: "备注内容：涉及第三章推导".into(),
        tags: vec!["阅读".into()],
        category_id: None,
        category_name: Some("学习".into()),
        is_focus: false,
    }
}

fn local_ms(hour: u32, minute: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(2026, 10, 29, hour, minute, 0)
        .single()
        .unwrap()
        .timestamp_millis()
}

fn provider() -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider {
        id: "ollama",
        base_url: "http://localhost:11434/v1".into(),
        model: "qwen2.5:7b".into(),
        api_key: None,
    }
}

fn chat_body(content: &str) -> String {
    json!({ "choices": [{ "message": { "content": content } }] }).to_string()
}

fn suggestions_json() -> &'static str {
    r##"[
        {"segmentId":"seg-1","title":"文献阅读：第三章","tags":["#阅读","#阅读","学习","","方法"],"categoryId":"cat-study","reason":"标题更具体"},
        {"segmentId":"seg-404","title":"不存在的段","tags":[],"categoryId":"cat-study","reason":""},
        {"segmentId":"seg-2","title":"  ","tags":[],"categoryId":"cat-unknown","reason":"bad"}
    ]"##
}

// ---------- 隐私模式（§8.3） ----------

#[test]
fn privacy_off_blocks_request() {
    let http = MockHttp::default();
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::Off,
    };
    let outcome = provider().classify(&http, &input);
    assert!(matches!(outcome, Err(AiError::PrivacyOff)));
    assert!(http.captured.lock().unwrap().is_empty());
}

#[test]
fn title_only_excludes_note() {
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body("[]")),
    );
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::TitleOnly,
    };
    provider().classify(&http, &input).unwrap();
    let body = http.captured.lock().unwrap()[0].body.clone().unwrap();
    assert!(body.contains("看文献"));
    assert!(body.contains("09:00"));
    assert!(!body.contains("第三章推导"));
}

#[test]
fn with_note_includes_note() {
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body("[]")),
    );
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::WithNote,
    };
    provider().classify(&http, &input).unwrap();
    let body = http.captured.lock().unwrap()[0].body.clone().unwrap();
    assert!(body.contains("第三章推导"));
}

#[test]
fn privacy_mode_default_is_title_only() {
    assert_eq!(PrivacyMode::from_setting(None), PrivacyMode::TitleOnly);
    assert_eq!(PrivacyMode::from_setting(Some("off")), PrivacyMode::Off);
}

// ---------- 请求形状 ----------

#[test]
fn empty_segments_skips_request() {
    let http = MockHttp::default();
    let input = ClassifyInput {
        segments: Vec::new(),
        categories: Vec::new(),
        privacy: PrivacyMode::TitleOnly,
    };
    assert!(provider().classify(&http, &input).unwrap().is_empty());
    assert!(http.captured.lock().unwrap().is_empty());
}

#[test]
fn request_targets_chat_completions() {
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body("[]")),
    );
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: vec![("cat-study".into(), "学习".into())],
        privacy: PrivacyMode::TitleOnly,
    };
    let with_key = OpenAiCompatibleProvider {
        id: "openai",
        base_url: "https://api.openai.com/v1".into(),
        model: "gpt-4o-mini".into(),
        api_key: Some("sk-test".into()),
    };
    with_key.classify(&http, &input).unwrap();
    let captured = http.captured.lock().unwrap();
    assert_eq!(
        captured[0].url,
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(captured[0].authorization.as_deref(), Some("Bearer sk-test"));
}

// ---------- 建议解析与清洗 ----------

#[test]
fn classify_parses_and_sanitizes_suggestions() {
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body(suggestions_json())),
    );
    let input = ClassifyInput {
        segments: vec![snapshot(), snapshot_with_id("seg-2")],
        categories: vec![("cat-study".into(), "学习".into())],
        privacy: PrivacyMode::TitleOnly,
    };
    let suggestions = provider().classify(&http, &input).unwrap();

    // seg-404 不在输入中，被过滤；剩 2 条。
    assert_eq!(suggestions.len(), 2);
    let first = &suggestions[0];
    assert_eq!(first.segment_id, "seg-1");
    assert_eq!(first.title.as_deref(), Some("文献阅读：第三章"));
    // 重复与空标签被清理。
    assert_eq!(
        first.tags,
        vec!["阅读".to_string(), "学习".to_string(), "方法".to_string()]
    );
    assert_eq!(first.category_id.as_deref(), Some("cat-study"));

    // seg-2：标题空白置空、未知分类置空。
    let second = &suggestions[1];
    assert_eq!(second.title, None);
    assert_eq!(second.category_id, None);
}

fn snapshot_with_id(id: &str) -> SegmentSnapshot {
    let mut segment = snapshot();
    segment.id = id.into();
    segment
}

#[test]
fn classify_tolerates_code_fences() {
    let fenced = format!("```json\n{}\n```", suggestions_json());
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body(&fenced)),
    );
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::TitleOnly,
    };
    let suggestions = provider().classify(&http, &input).unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].segment_id, "seg-1");
}

#[test]
fn classify_malformed_content_is_parse_error() {
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body("抱歉，我无法完成该任务。")),
    );
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::TitleOnly,
    };
    let outcome = provider().classify(&http, &input);
    assert!(matches!(outcome, Err(AiError::Parse(_))));
}

// ---------- 错误分类 ----------

#[test]
fn classify_maps_http_and_network_errors() {
    let input = ClassifyInput {
        segments: vec![snapshot()],
        categories: Vec::new(),
        privacy: PrivacyMode::TitleOnly,
    };

    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(401, r#"{"error":"bad key"}"#.into()),
    );
    let outcome = provider().classify(&http, &input);
    assert!(matches!(outcome, Err(AiError::Http(401, _))));

    let http = MockHttp::with_rule("/chat/completions", FakeResponse::Network);
    let outcome = provider().classify(&http, &input);
    assert!(matches!(outcome, Err(AiError::Network(_))));

    let http = MockHttp::with_rule("/chat/completions", FakeResponse::Timeout);
    let outcome = provider().classify(&http, &input);
    assert!(matches!(outcome, Err(AiError::Network(_))));
}

// ---------- 设置构建 ----------

#[test]
fn provider_from_settings_falls_back_to_defaults() {
    let ollama = OpenAiCompatibleProvider::from_settings("ollama", None, None, None);
    assert_eq!(ollama.base_url, "http://localhost:11434/v1");
    assert_eq!(ollama.model, "qwen2.5:7b");
    assert_eq!(ollama.id, "ollama");

    let custom = OpenAiCompatibleProvider::from_settings(
        "custom",
        Some("https://api.example.com/v1/"),
        Some("  "),
        Some("sk-1".into()),
    );
    assert_eq!(custom.base_url, "https://api.example.com/v1");
    assert_eq!(custom.model, "qwen2.5:7b");
    assert_eq!(custom.id, "custom");
}

fn review_input(privacy: PrivacyMode) -> dotlog_ai::review::ReviewInput {
    dotlog_ai::review::ReviewInput {
        from: "2026-10-29".into(),
        to: "2026-10-29".into(),
        question: "学习时间如何分配？".into(),
        segments: vec![snapshot()],
        privacy,
    }
}
#[test]
fn review_respects_privacy_and_returns_application_totals() {
    for privacy in [PrivacyMode::TitleOnly, PrivacyMode::WithNote] {
        let http = MockHttp::with_rule(
            "/chat/completions",
            FakeResponse::Http(200, chat_body("## 回顾\n依据记录安排复习。")),
        );
        let result = provider().review(&http, &review_input(privacy)).unwrap();
        assert_eq!(result.recorded_seconds, 4320);
        assert_eq!(result.record_count, 1);
        let capture = http.captured.lock().unwrap();
        let body = capture[0].body.as_ref().unwrap();
        assert_eq!(
            body.contains("第三章推导"),
            privacy == PrivacyMode::WithNote
        );
        assert!(body.contains("学习时间如何分配"));
        assert!(body.contains("2026-10-29"));
        assert!(result.markdown.starts_with("## 回顾"));
    }
}
#[test]
fn review_invalid_ranges_empty_and_privacy_off_do_not_send() {
    let http = MockHttp::default();
    assert!(matches!(
        provider().review(&http, &review_input(PrivacyMode::Off)),
        Err(AiError::PrivacyOff)
    ));
    for (from, to) in [
        ("2026-02-30", "2026-03-01"),
        ("2026-10-30", "2026-10-29"),
        ("2026-01-01", "2026-02-01"),
        ("2026-1-1", "2026-01-01"),
    ] {
        let mut input = review_input(PrivacyMode::TitleOnly);
        input.from = from.into();
        input.to = to.into();
        assert!(provider().review(&http, &input).is_err());
    }
    let mut input = review_input(PrivacyMode::TitleOnly);
    input.segments.clear();
    assert!(provider().review(&http, &input).is_err());
    input.segments = vec![snapshot(); 501];
    assert!(provider().review(&http, &input).is_err());
    input.segments = vec![snapshot()];
    input.question = "问".repeat(2001);
    assert!(provider().review(&http, &input).is_err());
    assert!(http.captured.lock().unwrap().is_empty());
}
#[test]
fn review_refuses_oversized_payload_and_empty_output() {
    let http = MockHttp::default();
    let mut input = review_input(PrivacyMode::WithNote);
    input.segments[0].note = "x".repeat(256 * 1024);
    assert!(provider().review(&http, &input).is_err());
    assert!(http.captured.lock().unwrap().is_empty());
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body("  ")),
    );
    assert!(matches!(
        provider().review(&http, &review_input(PrivacyMode::TitleOnly)),
        Err(AiError::Parse(_))
    ));
}
#[test]
fn model_list_validates_sorts_and_deduplicates_ids() {
    let http = MockHttp::with_rule(
        "/models",
        FakeResponse::Http(
            200,
            json!({"data":[{"id":"z"},{"id":"a"},{"id":"z"},{"id":""},{}]}).to_string(),
        ),
    );
    assert_eq!(provider().models(&http).unwrap(), vec!["a", "z"]);
    let http = MockHttp::with_rule("/models", FakeResponse::Http(200, "{}".into()));
    assert!(provider().models(&http).is_err());
}
#[test]
fn classification_deduplicates_records_before_display() {
    let row =
        json!({"segmentId":"seg-1","title":"阅读","tags":[],"categoryId":null,"reason":"整理"});
    let http = MockHttp::with_rule(
        "/chat/completions",
        FakeResponse::Http(200, chat_body(&json!([row.clone(), row]).to_string())),
    );
    assert_eq!(
        provider()
            .classify(
                &http,
                &ClassifyInput {
                    segments: vec![snapshot()],
                    categories: vec![],
                    privacy: PrivacyMode::TitleOnly
                }
            )
            .unwrap()
            .len(),
        1
    );
}
