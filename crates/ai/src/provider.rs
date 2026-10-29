//! AI 提供方：OpenAI 兼容 `chat/completions` 单实现（`docs/plugin-sdk.md` §8）。
//!
//! - 隐私三模式（§8.3）：`off` 不联网；`title_only` 仅标题/起止/分类标签；
//!   `with_note` 追加备注全文。载荷在发送前按模式裁剪。
//! - 超时 20 s；任何失败只返回错误，不影响本地数据。

use dotlog_plugin_sdk::{HttpClient, HttpRequest, PluginError, SegmentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::json;

const TIMEOUT_MS: u64 = 20_000;
const MAX_TAGS: usize = 5;

/// 隐私模式（§8.3）；设置缺失时默认 `title_only`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyMode {
    /// 不联网：任何请求直接拒绝。
    Off,
    /// 标题 + 起止 + 分类/标签（默认）。
    TitleOnly,
    /// 追加备注全文。
    WithNote,
}

impl PrivacyMode {
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("off") => PrivacyMode::Off,
            Some("with_note") => PrivacyMode::WithNote,
            _ => PrivacyMode::TitleOnly,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PrivacyMode::Off => "off",
            PrivacyMode::TitleOnly => "title_only",
            PrivacyMode::WithNote => "with_note",
        }
    }
}

/// 分类建议；只存在于内存，用户确认后由桌面层落库（§8.2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSuggestion {
    pub segment_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub category_id: Option<String>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ClassifyInput {
    pub segments: Vec<SegmentSnapshot>,
    /// 候选分类 (id, name)。
    pub categories: Vec<(String, String)>,
    pub privacy: PrivacyMode,
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI 联网已关闭（隐私模式 off）")]
    PrivacyOff,
    #[error("AI 未配置：{0}")]
    NotConfigured(String),
    #[error("AI 服务返回 HTTP {0}：{1}")]
    Http(u16, String),
    #[error("网络错误：{0}")]
    Network(String),
    #[error("响应解析失败：{0}")]
    Parse(String),
}

/// AI 提供方契约。失败不影响本地数据；调用方负责启停与隐私裁决。
pub trait AiProvider: Send + Sync {
    /// 提供方标识：`openai` / `ollama` / `custom`。
    fn id(&self) -> &'static str;

    /// 为一批段产出整理建议（标题/标签/分类）。
    fn classify(
        &self,
        http: &dyn HttpClient,
        input: &ClassifyInput,
    ) -> Result<Vec<AiSuggestion>, AiError>;
}

/// OpenAI 兼容实现；Ollama 以 `http://localhost:11434/v1` 复用同一端点。
pub struct OpenAiCompatibleProvider {
    /// `openai` / `ollama` / `custom`。
    pub id: &'static str,
    /// 已归一化（无尾部 `/`）的服务地址。
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl OpenAiCompatibleProvider {
    /// 按设置构建；未配置的项回落到提供方默认值。
    pub fn from_settings(
        provider: &str,
        base_url: Option<&str>,
        model: Option<&str>,
        api_key: Option<String>,
    ) -> Self {
        let (default_base, default_model, id) = match provider {
            "openai" => ("https://api.openai.com/v1", "gpt-4o-mini", "openai"),
            "ollama" => ("http://localhost:11434/v1", "qwen2.5:7b", "ollama"),
            _ => ("http://localhost:11434/v1", "qwen2.5:7b", "custom"),
        };
        Self {
            id,
            base_url: base_url
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(default_base)
                .trim_end_matches('/')
                .to_string(),
            model: model
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(default_model)
                .to_string(),
            api_key,
        }
    }
}

impl AiProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn classify(
        &self,
        http: &dyn HttpClient,
        input: &ClassifyInput,
    ) -> Result<Vec<AiSuggestion>, AiError> {
        if input.privacy == PrivacyMode::Off {
            return Err(AiError::PrivacyOff);
        }
        if input.segments.is_empty() {
            return Ok(Vec::new());
        }
        let user_payload = json!({
            "privacyMode": input.privacy.as_str(),
            "categories": input
                .categories
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect::<Vec<_>>(),
            "segments": input
                .segments
                .iter()
                .map(|segment| segment_payload(segment, input.privacy))
                .collect::<Vec<_>>(),
        });
        let body = json!({
            "model": self.model,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": system_prompt(&input.categories) },
                { "role": "user", "content": serde_json::to_string(&user_payload).map_err(|error| AiError::Parse(error.to_string()))? }
            ],
        });

        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if let Some(key) = &self.api_key {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }
        let url = format!("{}/chat/completions", self.base_url);
        let request = HttpRequest {
            method: "POST",
            url: &url,
            headers,
            body: Some(serde_json::to_vec(&body).map_err(|error| AiError::Parse(error.to_string()))?),
            timeout_ms: TIMEOUT_MS,
        };

        let response = http.send(request).map_err(map_send_error)?;
        if !response.is_success() {
            return Err(AiError::Http(response.status, snippet(&response.text())));
        }
        let content = extract_content(&response.text())?;
        let suggestions = parse_suggestions(&content)?;
        Ok(filter_valid(suggestions, input))
    }
}

/// 载荷按隐私模式裁剪：起止用本地 `HH:MM`，备注仅在 with_note 下发送。
fn segment_payload(segment: &SegmentSnapshot, privacy: PrivacyMode) -> serde_json::Value {
    let mut value = json!({
        "id": segment.id,
        "startAt": clock_text(segment.start_at_ms),
        "endAt": clock_text(segment.end_at_ms),
        "durationMinutes": segment.duration_seconds / 60,
        "title": segment.title,
        "tags": segment.tags,
        "categoryName": segment.category_name,
    });
    if privacy == PrivacyMode::WithNote && !segment.note.is_empty() {
        value["note"] = json!(segment.note);
    }
    value
}

fn system_prompt(categories: &[(String, String)]) -> String {
    let category_text = categories
        .iter()
        .map(|(id, name)| format!("{id}={name}"))
        .collect::<Vec<_>>()
        .join("，");
    format!(
        "你是时间记录整理助理。根据用户提供的时间段，为每段给出整理建议：\
更清晰的标题（不超过 20 字，去掉口语化表达）、1-3 个短标签、从候选分类中选择 categoryId（不确定则为 null）。\
只返回 JSON 数组，不要解释。元素格式：\
{{\"segmentId\":\"...\",\"title\":\"...\",\"tags\":[\"...\"],\"categoryId\":\"...\",\"reason\":\"一句话理由\"}}。\
候选分类：{category_text}。不存在的段不要输出。"
    )
}

fn map_send_error(error: PluginError) -> AiError {
    match error {
        PluginError::Network(reason) => AiError::Network(reason),
        PluginError::Timeout => AiError::Network("请求超时（20 s）".into()),
        other => AiError::Network(other.to_string()),
    }
}

/// 取 `choices[0].message.content`。
fn extract_content(body: &str) -> Result<String, AiError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| AiError::Parse(error.to_string()))?;
    value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string)
        .ok_or_else(|| AiError::Parse("缺少 choices[0].message.content".into()))
}

/// 解析模型输出；容错剥离代码围栏与非 JSON 前后缀。
fn parse_suggestions(content: &str) -> Result<Vec<AiSuggestion>, AiError> {
    let text = strip_code_fences(content);
    if let Ok(list) = serde_json::from_str::<Vec<AiSuggestion>>(text) {
        return Ok(sanitize(list));
    }
    if let Some(inner) = extract_array(text) {
        if let Ok(list) = serde_json::from_str::<Vec<AiSuggestion>>(&inner) {
            return Ok(sanitize(list));
        }
    }
    Err(AiError::Parse("模型输出不是 JSON 数组".into()))
}

fn strip_code_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let without_prefix = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    without_prefix.strip_suffix("```").unwrap_or(without_prefix).trim()
}

/// 兜底：截取首个 `[` 到最后一个 `]` 之间的内容。
fn extract_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    (start < end).then(|| text[start..=end].to_string())
}

/// 清洗：标签去 `#`、去重、限 5 个；标题去空白；丢弃空 segmentId。
fn sanitize(mut list: Vec<AiSuggestion>) -> Vec<AiSuggestion> {
    list.retain(|suggestion| !suggestion.segment_id.trim().is_empty());
    for suggestion in &mut list {
        suggestion.segment_id = suggestion.segment_id.trim().to_string();
        suggestion.title = suggestion
            .title
            .take()
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());
        let mut tags: Vec<String> = Vec::new();
        for tag in &suggestion.tags {
            let cleaned = tag.trim().trim_start_matches('#').trim().to_string();
            if !cleaned.is_empty() && !tags.contains(&cleaned) {
                tags.push(cleaned);
            }
        }
        tags.truncate(MAX_TAGS);
        suggestion.tags = tags;
    }
    list
}

/// 过滤：只保留本次输入的段；categoryId 不在候选内时置空。
fn filter_valid(suggestions: Vec<AiSuggestion>, input: &ClassifyInput) -> Vec<AiSuggestion> {
    let known_categories: Vec<&str> = input
        .categories
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    suggestions
        .into_iter()
        .filter(|suggestion| input.segments.iter().any(|segment| segment.id == suggestion.segment_id))
        .map(|mut suggestion| {
            let unknown = suggestion
                .category_id
                .as_deref()
                .is_some_and(|id| !known_categories.contains(&id));
            if unknown {
                suggestion.category_id = None;
            }
            suggestion
        })
        .collect()
}

fn snippet(body: &str) -> String {
    body.chars().take(120).collect()
}

/// 本地时区 `HH:MM`；无法换算时保留占位。
fn clock_text(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(time) => time.format("%H:%M").to_string(),
        None => "--:--".into(),
    }
}
