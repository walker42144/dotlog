//! DotLog 插件 SDK v1（`docs/plugin-sdk.md`）。
//!
//! 原则：核心不需要任何插件也能长期稳定运行。插件只能「消费事实、提出建议、
//! 发送外部数据」，不能触碰计时核心表——SDK 中不存在任何数据库或文件系统
//! 访问能力，所有外部资源都通过 host 注入的上下文对象访问。
//!
//! 接口按外部插件标准设计（v1 为编译进 host 的内置插件），避免二期重写。

use serde::{Deserialize, Serialize};

/// 与 host 严格匹配的契约版本；不一致的 manifest 拒绝加载（§2）。
pub const API_VERSION: u32 = 1;

/// 能力清单（§3）。未声明的能力对应上下文对象为 `None`，调用即失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    SegmentsRead,
    TagsWrite,
    Network,
    ClipboardRead,
    UiPanel,
    SecretStore,
    Importer,
    Exporter,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::SegmentsRead => "segments:read",
            Capability::TagsWrite => "tags:write",
            Capability::Network => "network",
            Capability::ClipboardRead => "clipboard:read",
            Capability::UiPanel => "ui:panel",
            Capability::SecretStore => "secret:store",
            Capability::Importer => "importer",
            Capability::Exporter => "exporter",
        }
    }
}

/// 插件清单（§2）。字段规则由 [`validate_manifest`] 校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    /// 小写 + 连字符，全局唯一，安装后不可更改。
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// 联网域名白名单（精确主机名）；空列表表示跟随插件设置中的动态地址，
    /// host 在每次请求时按插件配置的实例地址裁决。
    #[serde(default)]
    pub network_hosts: Vec<String>,
    /// JSON Schema 子集（string/number/boolean/enum），用于自动生成设置表单。
    #[serde(default)]
    pub settings_schema: serde_json::Value,
    /// v1 为 `builtin:<id>`。
    pub entry: String,
}

/// manifest 字段规则（§2）：apiVersion 严格匹配、id 格式合法、entry 非空。
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if manifest.api_version != API_VERSION {
        return Err(PluginError::Manifest(format!(
            "apiVersion {} 与 SDK {} 不兼容",
            manifest.api_version, API_VERSION
        )));
    }
    let id_ok =
        !manifest.id.is_empty() && manifest.id.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    if !id_ok {
        return Err(PluginError::Manifest(format!(
            "插件 id `{}` 只允许小写字母与连字符",
            manifest.id
        )));
    }
    if manifest.name.trim().is_empty() {
        return Err(PluginError::Manifest("name 不能为空".into()));
    }
    if !manifest.entry.starts_with("builtin:") {
        return Err(PluginError::Manifest(format!(
            "v1 的 entry 必须为 builtin:<id>，收到 `{}`",
            manifest.entry
        )));
    }
    Ok(())
}

/// 插件错误。所有上下文方法与生命周期钩子统一返回该类型。
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("能力未授权：{0}")]
    CapabilityDenied(&'static str),
    #[error("manifest 非法：{0}")]
    Manifest(String),
    #[error("插件调用超时")]
    Timeout,
    #[error("网络错误：{0}")]
    Network(String),
    #[error("配置缺失：{0}")]
    NotConfigured(String),
    #[error("存储错误：{0}")]
    Storage(String),
    #[error("{0}")]
    Other(String),
}

/// 关闭瞬间的不可变快照；插件延迟执行时读到的不是最新可变状态（§4）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentSnapshot {
    pub id: String,
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub duration_seconds: i64,
    pub title: String,
    pub note: String,
    pub tags: Vec<String>,
    pub category_id: Option<String>,
    pub category_name: Option<String>,
    pub is_focus: bool,
}

/// 段关闭的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Punch,
    Finish,
    SystemRecovery,
}

/// 唯一的核心事件入口（§4）：段关闭后由 host 分发。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentClosedEvent {
    pub segment: SegmentSnapshot,
    pub reason: CloseReason,
}

/// 插件向 outbox 投递的事件；重试与补偿统一交给 outbox（§4）。
#[derive(Debug, Clone)]
pub struct OutboxInput {
    /// 去重键，如 `memos:<segmentId>:publish`；重复入队直接忽略。
    pub dedupe_key: String,
    /// 如 `memos.publish`。
    pub event_type: String,
    /// 事件快照（不引用后续可变状态）。
    pub payload: serde_json::Value,
}

// ---------- host 注入的能力对象 ----------

#[derive(Debug)]
pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// 毫秒；host 可再施加更紧的上限。
    pub timeout_ms: u64,
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// `network` 能力。host 负责域名白名单校验与超时。
pub trait HttpClient: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError>;
}

/// `segments:read` 能力：只读段查询。
pub trait SegmentQuery: Send + Sync {
    fn segments_between(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<SegmentSnapshot>, PluginError>;
}

/// `tags:write` 能力：向段追加标签；host 校验能力并写审计记录。
pub trait TagWriter: Send + Sync {
    fn add_tag(&self, segment_id: &str, tag: &str) -> Result<(), PluginError>;
}

/// `secret:store` 能力：只允许读写 `dotlog.<pluginId>.*` 命名空间，
/// 底层为系统钥匙串（Windows Credential Manager）。
pub trait SecretStore: Send + Sync {
    fn get_secret(&self, key: &str) -> Result<Option<String>, PluginError>;
    fn set_secret(&self, key: &str, value: &str) -> Result<(), PluginError>;
    fn delete_secret(&self, key: &str) -> Result<(), PluginError>;
}

/// 本地可靠队列（始终可用，无需声明能力）。
pub trait OutboxSink: Send + Sync {
    fn enqueue(&self, event: OutboxInput) -> Result<(), PluginError>;
}

/// 只读本插件命名空间（`setting(scope='plugin:<id>')`）的配置。
pub trait PluginSettings: Send + Sync {
    fn get(&self, key: &str) -> Option<serde_json::Value>;
}

/// 插件日志；host 会附加插件 id 与耗时（§12.4）。
pub trait PluginLogger: Send + Sync {
    fn info(&self, message: &str);
    fn warn(&self, message: &str);
    fn error(&self, message: &str);
}

/// 外部对象回链（`integration_link` 表）：outbox handler 幂等与状态记录用。
#[derive(Debug, Clone)]
pub struct LinkRecord {
    pub segment_id: String,
    pub provider: String,
    pub external_id: Option<String>,
    /// `pending` / `published` / `failed` / `removed`。
    pub state: String,
    pub last_synced_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

pub trait IntegrationLinks: Send + Sync {
    fn get(&self, segment_id: &str, provider: &str) -> Result<Option<LinkRecord>, PluginError>;
    fn mark_published(
        &self,
        segment_id: &str,
        provider: &str,
        external_id: &str,
        now_ms: i64,
    ) -> Result<(), PluginError>;
    fn mark_failed(
        &self,
        segment_id: &str,
        provider: &str,
        error: &str,
        now_ms: i64,
    ) -> Result<(), PluginError>;
}

/// 插件上下文：能力逐项裁决，未声明的能力为 `None`（§4）。
pub struct PluginCtx<'a> {
    pub plugin_id: &'a str,
    pub segments: Option<&'a dyn SegmentQuery>,
    pub tags: Option<&'a dyn TagWriter>,
    pub http: Option<&'a dyn HttpClient>,
    pub secrets: Option<&'a dyn SecretStore>,
    pub outbox: &'a dyn OutboxSink,
    pub settings: &'a dyn PluginSettings,
    pub links: &'a dyn IntegrationLinks,
    pub log: &'a dyn PluginLogger,
    pub now_ms: i64,
}

/// 插件契约（§4）。所有方法 `&self`：可变状态由实现自行同步，
/// host 保证同一插件的调用串行执行。
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    /// 启动时调用一次；失败则插件标记为不可用，不影响应用启动。
    fn init(&self, _ctx: &PluginCtx) -> Result<(), PluginError> {
        Ok(())
    }

    /// 段关闭后的异步处理（唯一的核心事件入口）。
    fn on_segment_closed(
        &self,
        _ctx: &PluginCtx,
        _event: &SegmentClosedEvent,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn shutdown(&self) {}
}

// ---------- outbox 处理器扩展点 ----------

/// handler 的处理结论；退避与重试上限由 host 的 outbox worker 统一执行。
#[derive(Debug, Clone)]
pub enum OutboxOutcome {
    /// 成功；`external_id` 写入 `integration_link.external_id`（幂等回链）。
    Done { external_id: Option<String> },
    /// 可重试失败（网络、5xx）；按退避表重试。
    RetryLater { error: String },
    /// 不可恢复失败（4xx、配置缺失）；标记 failed 等待手动重试。
    Fail { error: String },
}

/// handler 运行环境：按插件能力裁决后注入。
pub struct OutboxEnv<'a> {
    pub http: Option<&'a dyn HttpClient>,
    pub secrets: Option<&'a dyn SecretStore>,
    pub settings: &'a dyn PluginSettings,
    pub links: &'a dyn IntegrationLinks,
    pub log: &'a dyn PluginLogger,
    pub now_ms: i64,
}

/// outbox 事件处理器；由集成 crate 实现，host 按 `event_type` 分发，
/// 并按 `plugin_id` 对应插件的 manifest 裁决能力。
pub trait OutboxHandler: Send + Sync {
    /// 所属插件 id（能力与设置命名空间）。
    fn plugin_id(&self) -> &str;
    fn event_type(&self) -> &'static str;
    fn handle(&self, payload: &serde_json::Value, env: &OutboxEnv) -> OutboxOutcome;
}
