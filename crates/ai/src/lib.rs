//! DotLog AI 助手（`docs/plugin-sdk.md` §8）。
//!
//! - 以内置插件身份注册进 host：出现在集成页，启停/设置/密钥复用 host 机制；
//! - 分类建议走 OpenAI 兼容 `chat/completions`（本地 Ollama 用
//!   `http://localhost:11434/v1` 直接复用），由桌面命令在用户触发时调用；
//! - 事实与建议分离（§8.2）：建议只在内存中返回，用户确认后由桌面层走
//!   `segment_update` 落库，本 crate 不接触数据库。

pub mod provider;

use dotlog_plugin_sdk::{API_VERSION, Capability, Plugin, PluginManifest};
use serde_json::json;

pub use provider::{
    AiError, AiProvider, AiSuggestion, ClassifyInput, OpenAiCompatibleProvider, PrivacyMode,
};

/// 插件 id（manifest 契约的一部分，安装后不可更改）。
pub const PLUGIN_ID: &str = "ai";

/// AI 助手内置插件：只声明能力，不响应段关闭事件（建议是用户触发的）。
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: PLUGIN_ID.into(),
            name: "AI 助手".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            capabilities: vec![Capability::Network, Capability::SecretStore],
            // 联网地址跟随设置中的 baseUrl（host 动态白名单）。
            network_hosts: Vec::new(),
            settings_schema: settings_schema(),
            entry: format!("builtin:{PLUGIN_ID}"),
        }
    }
}

fn settings_schema() -> serde_json::Value {
    json!({
        "provider": { "type": "string", "title": "提供方", "enum": ["openai", "ollama", "custom"], "default": "ollama" },
        "baseUrl": { "type": "string", "title": "服务地址", "default": "http://localhost:11434/v1", "description": "OpenAI 兼容端点；Ollama 默认 http://localhost:11434/v1" },
        "model": { "type": "string", "title": "模型", "default": "qwen2.5:7b" },
        "token": { "type": "string", "title": "API 密钥", "description": "存入系统钥匙串；本地 Ollama 可留空" },
        "privacyMode": { "type": "string", "title": "隐私模式", "enum": ["off", "title_only", "with_note"], "default": "title_only", "description": "off=不联网；title_only=仅标题/起止/分类标签；with_note=含备注全文" }
    })
}
