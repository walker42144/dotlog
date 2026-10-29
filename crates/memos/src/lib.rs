//! Memos 集成插件（`docs/plugin-sdk.md` §7）。
//!
//! - 段关闭后按 `autoPublish` 设置把发布事件写入 outbox（内容在快照时刻渲染，
//!   隐私模式立即生效）；
//! - 实际发送由 [`handler::PublishHandler`] 在 outbox worker 中执行，
//!   重试、退避与幂等回链统一交给 host；
//! - 单向发布（本地 → Memos），末尾隐藏注释保存 segment id 便于幂等。

pub mod browser;
pub mod endpoints;
pub mod handler;

pub use handler::PublishHandler;

use dotlog_plugin_sdk::{
    Capability, OutboxInput, Plugin, PluginCtx, PluginError, PluginManifest, SegmentClosedEvent,
    SegmentSnapshot, API_VERSION,
};
use serde_json::json;

use crate::endpoints::normalize_base;

/// 插件 id（manifest 契约的一部分，安装后不可更改）。
pub const PLUGIN_ID: &str = "memos";
/// outbox 事件类型；host 取 `.` 前缀作为回链 provider。
pub const EVENT_TYPE: &str = "memos.publish";
/// 发布模板默认值（§7.3）。
pub const LEGACY_DEFAULT_TEMPLATE: &str =
    "**{start}–{end}（{duration}）** {title}\n{note}\n{tags}\n<!-- dotlog:segment={segmentId} -->";
pub const DEFAULT_TEMPLATE: &str = "# 打点记录 · {title}\n\n| 项目 | 记录 |\n| --- | --- |\n| 开始 | {startDateTime} |\n| 结束 | {endDateTime} |\n| 有效时长 | {activeDuration} |\n| 暂停合计 | {paused} |\n| 跨度（含暂停） | {elapsed} |\n| 分类 | {category} |\n| 专注 | {focus} |\n\n**标签**：{tags}\n\n## 备注\n\n{note}\n\n#打点记录\n\n<!-- dotlog:segment={segmentId} -->";

/// Memos 内置插件：负责「段关闭 → outbox 入队」这一侧契约。
pub struct MemosPlugin;

impl Plugin for MemosPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: PLUGIN_ID.into(),
            name: "Memos 发布".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            // 与 §2 示例一致；network_hosts 留空表示跟随设置中的实例地址。
            capabilities: vec![
                Capability::SegmentsRead,
                Capability::Network,
                Capability::SecretStore,
                Capability::UiPanel,
            ],
            network_hosts: Vec::new(),
            settings_schema: settings_schema(),
            entry: format!("builtin:{PLUGIN_ID}"),
        }
    }

    fn on_segment_closed(
        &self,
        ctx: &PluginCtx,
        event: &SegmentClosedEvent,
    ) -> Result<(), PluginError> {
        let auto_publish = ctx.settings.get("autoPublish");
        let auto = auto_publish
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("off");
        let enabled = match auto {
            "on_close" => true,
            "tag_rule" => {
                tag_rule_matches(ctx.settings.get("tagRules").as_ref(), &event.segment.tags)
            }
            _ => false,
        };
        if !enabled {
            return Ok(());
        }
        // 未配置有效实例地址时跳过并记日志；配置可能稍后完成，不算插件错误。
        let instance_url = ctx.settings.get("instanceUrl");
        let instance_valid = instance_url
            .as_ref()
            .and_then(|value| value.as_str())
            .and_then(normalize_base)
            .is_some();
        if !instance_valid {
            ctx.log.warn("未配置有效的实例地址，跳过自动发布");
            return Ok(());
        }
        let template_setting = ctx.settings.get("template");
        let template = template_setting
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or(DEFAULT_TEMPLATE);
        let include_note = ctx
            .settings
            .get("includeNote")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let content = render_content(&event.segment, template, include_note);
        ctx.outbox.enqueue(OutboxInput {
            dedupe_key: format!("{PLUGIN_ID}:{}:publish", event.segment.id),
            event_type: EVENT_TYPE.into(),
            payload: json!({
                "segmentId": event.segment.id,
                "title": event.segment.title,
                "content": content,
            }),
        })
    }
}

/// `tag_rule` 模式：配置了标签列表则命中任一即发布，未配置时有标签即发布。
fn tag_rule_matches(rule: Option<&serde_json::Value>, tags: &[String]) -> bool {
    let allowed: Vec<&str> = rule
        .and_then(|value| value.as_str())
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if allowed.is_empty() {
        return !tags.is_empty();
    }
    tags.iter()
        .any(|tag| allowed.iter().any(|rule| rule.eq_ignore_ascii_case(tag)))
}

/// 按模板渲染发布内容，保留备注段落；内置模板附带隐藏回链标识。
pub fn render_content(snapshot: &SegmentSnapshot, template: &str, include_note: bool) -> String {
    let detailed = template.trim().is_empty()
        || template == LEGACY_DEFAULT_TEMPLATE
        || template == DEFAULT_TEMPLATE;
    let template = if detailed { DEFAULT_TEMPLATE } else { template };
    let note = if include_note {
        if detailed && snapshot.note.trim().is_empty() {
            "未填写备注"
        } else {
            snapshot.note.as_str()
        }
    } else if detailed {
        "备注未同步（可在“集成 → Memos”开启“包含备注”）"
    } else {
        ""
    };
    let elapsed = ((snapshot.end_at_ms - snapshot.start_at_ms) / 1000).max(0);
    let paused = (elapsed - snapshot.duration_seconds).max(0);
    let tags = tags_line(&snapshot.tags);
    let values = [
        ("start", clock_text(snapshot.start_at_ms)),
        ("end", clock_text(snapshot.end_at_ms)),
        ("startDateTime", date_time_text(snapshot.start_at_ms)),
        ("endDateTime", date_time_text(snapshot.end_at_ms)),
        ("duration", duration_text(snapshot.duration_seconds)),
        (
            "activeDuration",
            readable_duration(snapshot.duration_seconds),
        ),
        ("elapsed", readable_duration(elapsed)),
        ("paused", readable_duration(paused)),
        (
            "title",
            if detailed && snapshot.title.trim().is_empty() {
                "未命名记录".into()
            } else {
                snapshot.title.trim().into()
            },
        ),
        (
            "category",
            snapshot
                .category_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("未分类")
                .replace('|', "\\|")
                .replace(['\n', '\r'], " "),
        ),
        ("focus", if snapshot.is_focus { "是" } else { "否" }.into()),
        ("note", note.into()),
        (
            "tags",
            if detailed && tags.is_empty() {
                "未添加标签".into()
            } else {
                tags
            },
        ),
        ("segmentId", snapshot.id.clone()),
    ];
    // Substitute only the original template, never interpret placeholders in a user's note.
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            result.push_str(&rest[start..]);
            return result;
        };
        let key = &rest[start + 1..start + end];
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            result.push_str(value);
        } else {
            result.push_str(&rest[start..=start + end]);
        }
        rest = &rest[start + end + 1..];
    }
    result.push_str(rest);
    result.trim().to_string()
}

fn date_time_text(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "时间未知".into())
}

fn readable_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let mut parts = Vec::new();
    if seconds >= 3600 {
        parts.push(format!("{} 小时", seconds / 3600));
    }
    if seconds % 3600 >= 60 {
        parts.push(format!("{} 分", seconds % 3600 / 60));
    }
    if seconds % 60 > 0 || parts.is_empty() {
        parts.push(format!("{} 秒", seconds % 60));
    }
    parts.join(" ")
}

/// 本地时区的 `HH:MM`；无法换算时保留占位。
fn clock_text(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(time) => time.format("%H:%M").to_string(),
        None => "--:--".into(),
    }
}

/// `1h12m` / `45m` 风格的时长文本。
fn duration_text(seconds: i64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn tags_line(tags: &[String]) -> String {
    tags.iter()
        .map(|tag| format!("#{tag}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 设置表单 schema（JSON Schema 子集，§2）；token 实际存系统钥匙串。
fn settings_schema() -> serde_json::Value {
    json!({
        "instanceUrl": { "type": "string", "title": "实例地址", "description": "如 https://memos.example.com" },
        "token": { "type": "string", "title": "访问令牌", "description": "存入系统钥匙串，不明文落库" },
        "visibility": { "type": "string", "title": "可见性", "enum": ["private", "protected", "public"], "default": "private" },
        "autoPublish": { "type": "string", "title": "自动发布", "enum": ["off", "on_close", "tag_rule"], "default": "off" },
        "tagRules": { "type": "string", "title": "标签规则", "description": "逗号分隔的标签；tag_rule 模式下命中即发布", "default": "" },
        "includeNote": { "type": "boolean", "title": "包含备注", "description": "默认不发备注；完整记录中的备注全文由此开关控制", "default": false },
        "template": { "type": "string", "title": "发布模板", "default": DEFAULT_TEMPLATE }
    })
}
