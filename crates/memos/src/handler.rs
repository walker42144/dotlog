//! `memos.publish` 的 outbox 处理器（`docs/plugin-sdk.md` §6–§7）。
//!
//! 结论分类：2xx → Done（host 写回链）；4xx / 配置缺失 → Fail（等手动重试）；
//! 5xx / 网络 / 钥匙串故障 → RetryLater（退避重试）。

use dotlog_plugin_sdk::{HttpRequest, OutboxEnv, OutboxHandler, OutboxOutcome, PluginError};

use crate::endpoints::{normalize_base, publish_path, visibility_proto};
use crate::{render_content, DEFAULT_TEMPLATE, EVENT_TYPE, PLUGIN_ID};

const TIMEOUT_MS: u64 = 10_000;

/// 把段快照发布到 Memos 实例。
pub struct PublishHandler;

impl OutboxHandler for PublishHandler {
    fn plugin_id(&self) -> &str {
        PLUGIN_ID
    }

    fn event_type(&self) -> &'static str {
        EVENT_TYPE
    }

    fn handle(&self, payload: &serde_json::Value, env: &OutboxEnv) -> OutboxOutcome {
        let Some(segment_id) = payload.get("segmentId").and_then(|value| value.as_str()) else {
            return OutboxOutcome::Fail {
                error: "payload 缺少 segmentId".into(),
            };
        };
        let Some(content) = payload.get("content").and_then(|value| value.as_str()) else {
            return OutboxOutcome::Fail {
                error: "payload 缺少 content".into(),
            };
        };

        let live_content;
        let content = if let Some(query) = env.segments {
            let segment = match query.segment_by_id(segment_id) {
                Ok(Some(s)) if s.end_at_ms > s.start_at_ms => s,
                Ok(_) => {
                    return OutboxOutcome::Fail {
                        error: "记录已撤销、删除或重新开始，已取消发布".into(),
                    }
                }
                Err(e) => return retry(e.to_string()),
            };
            let template = env.settings.get("template");
            let include_note = env
                .settings
                .get("includeNote")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            live_content = render_content(
                &segment,
                template
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_TEMPLATE),
                include_note,
            );
            live_content.as_str()
        } else {
            content
        };

        // 幂等（§6）：已发布的事件直接成功，不再发送。
        match env.links.get(segment_id, PLUGIN_ID) {
            Ok(Some(link)) if link.state == "published" => {
                env.log
                    .info(&format!("段 {segment_id} 已发布，跳过重复发送"));
                return OutboxOutcome::Done {
                    external_id: link.external_id,
                };
            }
            Ok(_) => {}
            Err(error) => {
                return retry(format!("回链状态读取失败：{error}"));
            }
        }

        // 发送配置读当前设置：地址与可见性允许在事件排队后修改。
        let instance_url = env.settings.get("instanceUrl");
        let Some(base) = instance_url
            .as_ref()
            .and_then(|value| value.as_str())
            .and_then(normalize_base)
        else {
            return OutboxOutcome::Fail {
                error: "缺少有效实例地址（instanceUrl）".into(),
            };
        };
        let visibility_setting = env.settings.get("visibility");
        let visibility = match visibility_setting.as_ref().and_then(|value| value.as_str()) {
            Some("protected") => "protected",
            Some("public") => "public",
            _ => "private",
        };
        let api_version_setting = env.settings.get("apiVersion");
        let api_version = api_version_setting
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("v1");
        let Some(path) = publish_path(api_version) else {
            return OutboxOutcome::Fail {
                error: format!("不支持的 Memos API 版本：{api_version}"),
            };
        };

        let Some(secrets) = env.secrets else {
            return OutboxOutcome::Fail {
                error: "未授权 secret:store 能力".into(),
            };
        };
        let token = match secrets.get_secret("token") {
            Ok(Some(token)) => token,
            Ok(None) => {
                return OutboxOutcome::Fail {
                    error: "缺少访问令牌，请先在集成页配置".into(),
                };
            }
            Err(error) => return retry(format!("读取令牌失败：{error}")),
        };

        let Some(http) = env.http else {
            return OutboxOutcome::Fail {
                error: "未授权 network 能力".into(),
            };
        };
        let body = match serde_json::to_vec(&serde_json::json!({
            "content": content,
            "visibility": visibility_proto(visibility),
        })) {
            Ok(body) => body,
            Err(error) => {
                return OutboxOutcome::Fail {
                    error: format!("payload 序列化失败：{error}"),
                };
            }
        };

        let url = format!("{base}{path}");
        let request = HttpRequest {
            method: "POST",
            url: &url,
            headers: vec![
                ("Authorization".to_string(), format!("Bearer {token}")),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            body: Some(body),
            timeout_ms: TIMEOUT_MS,
        };
        match http.send(request) {
            Ok(response) if response.is_success() => {
                let external_id = memo_external_id(&response.text());
                if external_id.is_none() {
                    return OutboxOutcome::Fail {
                        error: "服务未返回有效的 Memos 记录标识，未标记成功，请核对远端后重试"
                            .into(),
                    };
                }
                env.log.info(&format!("段 {segment_id} 发布成功"));
                OutboxOutcome::Done { external_id }
            }
            Ok(response) if (400..500).contains(&response.status) => OutboxOutcome::Fail {
                error: format!(
                    "Memos 拒绝请求（HTTP {}）：{}",
                    response.status,
                    body_snippet(&response.text())
                ),
            },
            Ok(response) => retry(format!("Memos 服务器错误（HTTP {}）", response.status)),
            Err(error @ (PluginError::Network(_) | PluginError::Timeout)) => {
                retry(error.to_string())
            }
            Err(error) => OutboxOutcome::Fail {
                error: error.to_string(),
            },
        }
    }
}

fn retry(error: String) -> OutboxOutcome {
    OutboxOutcome::RetryLater { error }
}

/// 从 2xx 响应提取外部 id：优先 `name`（如 `memos/123`），其次 `uid`。
fn memo_external_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("name")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("uid").and_then(|value| value.as_str()))
        .map(str::to_string)
}

/// 错误响应正文截断，避免日志里塞满页面内容。
fn body_snippet(body: &str) -> String {
    body.chars().take(120).collect()
}
