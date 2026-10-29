//! M5 集成装配：插件宿主、outbox worker 与集成/AI 命令
//! （`docs/ipc-contract.md` §3.5/§4，`docs/plugin-sdk.md` §5–§8）。

use std::collections::BTreeMap;
use std::sync::Arc;

use dotlog_ai::{
    AiError, AiPlugin, AiProvider, AiSuggestion, ClassifyInput, OpenAiCompatibleProvider,
    PrivacyMode,
};
use dotlog_application::dto::{BackfillInput, CategoryView, SegmentPatch, SegmentView};
use dotlog_application::{outbox_state, TimerStore};
use dotlog_memos::endpoints as memos_endpoints;
use dotlog_memos::{MemosPlugin, PublishHandler};
use dotlog_plugin_host::host::{segment_snapshot, HostLogger, PluginHost, TICK_INTERVAL};
use dotlog_plugin_host::net::{host_of, KeyringStore, UreqClient};
use dotlog_plugin_sdk::{CloseReason, HttpClient, PluginManifest, SecretStore, SegmentClosedEvent};
use dotlog_tomatodo::{
    self as tomatodo, CompareResult, DotlogSegment, ImportResult, ScanResult, TomatodoPlugin,
    compare, resolve_db_path, scan,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{app_dir, log, AppState, IpcError, IpcResult, store_error};

/// 同步状态事件（ipc-contract §4）。
const INTEGRATION_EVENT: &str = "integration://status";

/// host 日志出口：并入每日应用日志，带插件 id（§12.4）。
struct DesktopHostLogger;

impl HostLogger for DesktopHostLogger {
    fn log(&self, level: &str, plugin_id: &str, message: &str) {
        log::write(&app_dir(), level, &format!("[{plugin_id}] {message}"));
    }
}

/// 组装插件宿主：注册内置插件与 outbox 处理器。
pub fn build_host(store: Arc<dyn TimerStore>) -> Arc<PluginHost> {
    let host = Arc::new(PluginHost::new(store, Arc::new(DesktopHostLogger)));
    host.register(Arc::new(MemosPlugin));
    host.register_handler(Arc::new(PublishHandler));
    host.register(Arc::new(AiPlugin));
    host.register(Arc::new(TomatodoPlugin));
    host
}

/// outbox worker：先补发一次（应用关闭期间到期的任务），再按 30 s 轮询。
pub fn spawn_worker(host: Arc<PluginHost>, app: AppHandle) {
    std::thread::spawn(move || {
        host.run_outbox_tick(now_ms());
        loop {
            std::thread::sleep(TICK_INTERVAL);
            if host.run_outbox_tick(now_ms()) > 0 {
                emit_status(&app);
            }
        }
    });
}

/// 段关闭后分发给所有启用的插件；不阻塞打点命令（§5）。
pub fn dispatch_closed(state: &AppState, segment_id: &str, reason: CloseReason) {
    let segment = match state.store.find_segment(segment_id) {
        Ok(Some(segment)) => segment,
        _ => return,
    };
    let categories = state.store.categories().unwrap_or_default();
    let event = SegmentClosedEvent {
        segment: segment_snapshot(&segment, &categories),
        reason,
    };
    state.host.dispatch_segment_closed(&event);
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

fn emit_status(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    for manifest in state.host.manifests() {
        let view = integration_view(&state, &manifest);
        let _ = app.emit(INTEGRATION_EVENT, &view);
    }
}

// ---------- 集成视图 ----------

/// 集成卡片（ipc-contract §3.5 `IntegrationView`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub configured: bool,
    pub capabilities: Vec<String>,
    pub last_error: Option<String>,
    pub last_sync_at: Option<i64>,
}

/// 必填配置键：缺失即视为未配置。
fn required_config_key(plugin_id: &str) -> &'static str {
    match plugin_id {
        "memos" => "instanceUrl",
        "ai" => "baseUrl",
        // 番茄Todo 用默认路径即可，无需必填配置。
        "tomatodo" => "",
        _ => "",
    }
}

fn plugin_settings(state: &AppState, plugin_id: &str) -> serde_json::Map<String, serde_json::Value> {
    state
        .timer
        .settings(Some(&format!("plugin:{plugin_id}")))
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// 集成的最近状态：错误取失败队列最新一条，同步时间取已完成最新一条。
fn provider_status(state: &AppState, provider: &str) -> (Option<String>, Option<i64>) {
    let latest_error = state
        .store
        .outbox_list(Some(outbox_state::FAILED), 50)
        .ok()
        .and_then(|records| latest_for_provider(&records, provider))
        .and_then(|record| record.last_error);
    let last_sync_at = state
        .store
        .outbox_list(Some(outbox_state::DONE), 50)
        .ok()
        .and_then(|records| latest_for_provider(&records, provider))
        .map(|record| record.updated_at_ms);
    (latest_error, last_sync_at)
}

fn latest_for_provider(
    records: &[dotlog_application::OutboxRecord],
    provider: &str,
) -> Option<dotlog_application::OutboxRecord> {
    records
        .iter()
        .filter(|record| record.event_type.split('.').next() == Some(provider))
        .max_by_key(|record| record.updated_at_ms)
        .cloned()
}

fn integration_view(state: &AppState, manifest: &PluginManifest) -> IntegrationView {
    let settings = plugin_settings(state, &manifest.id);
    let configured = settings
        .get(required_config_key(&manifest.id))
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let (last_error, last_sync_at) = provider_status(state, &manifest.id);
    IntegrationView {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        enabled: state.host.plugin_enabled(&manifest.id),
        configured,
        capabilities: manifest.capabilities.iter().map(|cap| cap.as_str().to_string()).collect(),
        last_error,
        last_sync_at,
    }
}

// ---------- 集成命令 ----------

#[tauri::command]
pub fn integration_list(state: tauri::State<'_, AppState>) -> IpcResult<Vec<IntegrationView>> {
    Ok(state
        .host
        .manifests()
        .iter()
        .map(|manifest| integration_view(&state, manifest))
        .collect())
}

#[tauri::command]
pub fn integration_set_enabled(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> IpcResult<IntegrationView> {
    let manifest = manifest_of(&state, &id)?;
    state.host.set_enabled(&id, enabled, now_ms());
    log::write(&app_dir(), "INFO", &format!("集成 {id} 启停：{enabled}"));
    let view = integration_view(&state, &manifest);
    let _ = app.emit(INTEGRATION_EVENT, &view);
    Ok(view)
}

#[tauri::command]
pub fn integration_config_set(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    config: BTreeMap<String, serde_json::Value>,
) -> IpcResult<IntegrationView> {
    let manifest = manifest_of(&state, &id)?;
    // 实例地址做基本格式校验（§7.1）；完整可达性由 integration_test 验证。
    for (key, value) in &config {
        if (key == "instanceUrl" || key == "baseUrl")
            && value
                .as_str()
                .map(|url| {
                    let url = url.trim();
                    !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://")
                })
                .unwrap_or(false)
        {
            return Err(store_error(
                "E_SETTING_INVALID",
                format!("{key} 需要以 http(s):// 开头"),
            ));
        }
    }
    let scope = format!("plugin:{id}");
    let entries: Vec<(String, serde_json::Value)> = config.into_iter().collect();
    state
        .timer
        .settings_patch(&scope, entries)
        .map_err(IpcError::from)?;
    log::write(&app_dir(), "INFO", &format!("集成 {id} 配置已更新"));
    let view = integration_view(&state, &manifest);
    let _ = app.emit(INTEGRATION_EVENT, &view);
    Ok(view)
}

#[tauri::command]
pub fn integration_secret_set(
    state: tauri::State<'_, AppState>,
    id: String,
    secret: String,
) -> IpcResult<()> {
    manifest_of(&state, &id)?;
    let store = KeyringStore::for_plugin(&id);
    let outcome = if secret.trim().is_empty() {
        store.delete_secret("token")
    } else {
        store.set_secret("token", secret.trim())
    };
    outcome.map_err(|error| store_error("E_UNKNOWN", error.to_string()))?;
    log::write(&app_dir(), "INFO", &format!("集成 {id} 令牌已更新"));
    Ok(())
}

/// `integration_test` 的返回。
#[derive(Debug, Serialize)]
pub struct IntegrationTestResult {
    pub ok: bool,
    pub message: String,
}

#[tauri::command]
pub fn integration_test(
    state: tauri::State<'_, AppState>,
    id: String,
) -> IpcResult<IntegrationTestResult> {
    manifest_of(&state, &id)?;
    let settings = plugin_settings(&state, &id);
    let result = match id.as_str() {
        "memos" => test_memos(&state, &settings),
        "ai" => test_ai(&settings),
        _ => Err(store_error("E_UNKNOWN", format!("未知集成：{id}"))),
    };
    match result {
        Ok(message) => Ok(IntegrationTestResult { ok: true, message }),
        Err(error) => Ok(IntegrationTestResult {
            ok: false,
            message: error.message,
        }),
    }
}

/// Memos 连接测试：探测版本并保存 `apiVersion`（§7.2）。
fn test_memos(
    state: &AppState,
    settings: &serde_json::Map<String, serde_json::Value>,
) -> IpcResult<String> {
    let base_url = settings
        .get("instanceUrl")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| store_error("E_INTEGRATION_NOT_CONFIGURED", "请先填写实例地址".to_string()))?;
    let token = KeyringStore::for_plugin("memos")
        .get_secret("token")
        .map_err(|error| store_error("E_UNKNOWN", error.to_string()))?;
    let client = UreqClient {
        allowed_hosts: host_of(base_url).into_iter().collect(),
        timeout_ms: 10_000,
    };
    match memos_endpoints::probe(&client, base_url, token.as_deref()) {
        Ok(outcome) => {
            let _ = state.timer.settings_patch(
                "plugin:memos",
                vec![("apiVersion".into(), serde_json::json!(outcome.api_version()))],
            );
            log::write(&app_dir(), "INFO", &format!("Memos 连接测试成功：{base_url}"));
            Ok(outcome.describe().to_string())
        }
        Err(message) => Err(store_error("E_NETWORK", message)),
    }
}

/// AI 连接测试：请求 OpenAI 兼容的 `GET /models`。
fn test_ai(settings: &serde_json::Map<String, serde_json::Value>) -> IpcResult<String> {
    let base_url = settings
        .get("baseUrl")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| store_error("E_INTEGRATION_NOT_CONFIGURED", "请先填写服务地址".to_string()))?;
    let api_key = KeyringStore::for_plugin("ai")
        .get_secret("token")
        .map_err(|error| store_error("E_UNKNOWN", error.to_string()))?
        .filter(|value| !value.is_empty());
    let url = format!("{}/models", base_url.trim().trim_end_matches('/'));
    let client = UreqClient {
        allowed_hosts: host_of(&url).into_iter().collect(),
        timeout_ms: 10_000,
    };
    let headers = api_key
        .map(|key| vec![("Authorization".to_string(), format!("Bearer {key}"))])
        .unwrap_or_default();
    let request = dotlog_plugin_sdk::HttpRequest {
        method: "GET",
        url: &url,
        headers,
        body: None,
        timeout_ms: 10_000,
    };
    match client.send(request) {
        Ok(response) if response.is_success() => Ok("连接成功".to_string()),
        Ok(response) => Err(store_error(
            "E_NETWORK",
            format!("服务返回 HTTP {}", response.status),
        )),
        Err(error) => Err(store_error("E_NETWORK", error.to_string())),
    }
}

fn manifest_of(state: &AppState, id: &str) -> IpcResult<PluginManifest> {
    state
        .host
        .manifests()
        .into_iter()
        .find(|manifest| manifest.id == id)
        .ok_or_else(|| store_error("E_UNKNOWN", format!("未知集成：{id}")))
}

// ---------- outbox 命令 ----------

/// outbox 队列条目（ipc-contract §3.5 `OutboxView`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxView {
    pub id: String,
    pub event_type: String,
    pub state: String,
    pub attempts: i64,
    pub next_retry_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// payload 中的段标题，仅用于列表展示。
    pub title: Option<String>,
}

impl From<dotlog_application::OutboxRecord> for OutboxView {
    fn from(record: dotlog_application::OutboxRecord) -> Self {
        let title = serde_json::from_str::<serde_json::Value>(&record.payload)
            .ok()
            .and_then(|payload| {
                payload
                    .get("title")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            });
        Self {
            id: record.id,
            event_type: record.event_type,
            state: record.state,
            attempts: record.attempts,
            next_retry_at_ms: record.next_retry_at_ms,
            last_error: record.last_error,
            created_at_ms: record.created_at_ms,
            updated_at_ms: record.updated_at_ms,
            title,
        }
    }
}

#[tauri::command]
pub fn outbox_list(
    app_state: tauri::State<'_, AppState>,
    state: Option<String>,
) -> IpcResult<Vec<OutboxView>> {
    let records = app_state
        .store
        .outbox_list(state.as_deref(), 100)
        .map_err(IpcError::from)?;
    Ok(records.into_iter().map(OutboxView::from).collect())
}

#[tauri::command]
pub fn outbox_retry(
    app: AppHandle,
    app_state: tauri::State<'_, AppState>,
    id: String,
) -> IpcResult<OutboxView> {
    app_state
        .store
        .outbox_retry(&id, now_ms())
        .map_err(IpcError::from)?;
    // 手动重试后立即处理一次（§6），不等下一个 30 s 周期。
    app_state.host.run_outbox_tick(now_ms());
    emit_status(&app);
    let records = app_state
        .store
        .outbox_list(None, 200)
        .map_err(IpcError::from)?;
    records
        .into_iter()
        .find(|record| record.id == id)
        .map(OutboxView::from)
        .ok_or_else(|| store_error("E_UNKNOWN", "事件不存在".to_string()))
}

// ---------- AI 命令 ----------

#[tauri::command]
pub fn ai_suggest(
    app_state: tauri::State<'_, AppState>,
    date: Option<String>,
) -> IpcResult<Vec<AiSuggestion>> {
    if !app_state.host.plugin_enabled(dotlog_ai::PLUGIN_ID) {
        return Err(store_error("E_PLUGIN_DISABLED", "AI 助手未启用".to_string()));
    }
    let settings = plugin_settings(&app_state, dotlog_ai::PLUGIN_ID);
    let privacy = PrivacyMode::from_setting(settings.get("privacyMode").and_then(|v| v.as_str()));
    if privacy == PrivacyMode::Off {
        return Err(store_error("E_PLUGIN_DISABLED", "AI 联网已关闭（隐私模式 off）".to_string()));
    }
    let provider = ai_provider(&settings)?;

    // 待整理段：指定日（默认今天）的已关闭且未忽略段（§8.3 title_only 范围）。
    let day = app_state.timer.day(date.as_deref()).map_err(IpcError::from)?;
    let categories_rows = app_state.store.categories().map_err(IpcError::from)?;
    let segments = app_state
        .store
        .day_segments(day.window_start, day.window_end)
        .map_err(IpcError::from)?
        .iter()
        .filter(|segment| !segment.is_active() && !segment.is_ignored)
        .map(|segment| segment_snapshot(segment, &categories_rows))
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Ok(Vec::new());
    }
    let categories = categories_rows
        .iter()
        .map(|row| (row.id.clone(), row.name.clone()))
        .collect::<Vec<_>>();

    let base = provider.base_url.clone();
    let client = UreqClient {
        allowed_hosts: host_of(&base).into_iter().collect(),
        timeout_ms: 20_000,
    };
    let input = ClassifyInput {
        segments,
        categories,
        privacy,
    };
    provider.classify(&client, &input).map_err(ai_error)
}

fn ai_provider(
    settings: &serde_json::Map<String, serde_json::Value>,
) -> IpcResult<OpenAiCompatibleProvider> {
    let provider_kind = settings
        .get("provider")
        .and_then(|value| value.as_str())
        .unwrap_or("ollama");
    let base_url = settings.get("baseUrl").and_then(|value| value.as_str());
    let model = settings.get("model").and_then(|value| value.as_str());
    let api_key = KeyringStore::for_plugin(dotlog_ai::PLUGIN_ID)
        .get_secret("token")
        .map_err(|error| store_error("E_UNKNOWN", error.to_string()))?
        .filter(|value| !value.is_empty());
    Ok(OpenAiCompatibleProvider::from_settings(
        provider_kind,
        base_url,
        model,
        api_key,
    ))
}

fn ai_error(error: AiError) -> IpcError {
    let code = match &error {
        AiError::PrivacyOff => "E_PLUGIN_DISABLED",
        AiError::NotConfigured(_) => "E_INTEGRATION_NOT_CONFIGURED",
        AiError::Http(_, _) | AiError::Network(_) => "E_NETWORK",
        AiError::Parse(_) => "E_UNKNOWN",
    };
    store_error(code, error.to_string())
}

/// `ai_apply` 入参：用户确认后的建议子集；`category_id` 传空串表示清空。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiApplyInput {
    pub segment_id: String,
    pub revision: u32,
    pub title: Option<String>,
    pub category_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[tauri::command]
pub fn ai_apply(
    app_state: tauri::State<'_, AppState>,
    input: AiApplyInput,
) -> IpcResult<SegmentView> {
    // 先改标题/分类（乐观锁用传入 revision），标签随后追加（每次会推进 revision）。
    let patch = SegmentPatch {
        title: input
            .title
            .as_ref()
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty()),
        note: None,
        start_at: None,
        end_at: None,
        category_id: input.category_id.as_ref().map(|id| {
            let id = id.trim();
            if id.is_empty() {
                None
            } else {
                Some(id.to_string())
            }
        }),
        is_focus: None,
        is_ignored: None,
    };
    let view = if patch.title.is_some() || patch.category_id.is_some() {
        app_state
            .timer
            .update_segment(&input.segment_id, input.revision, patch)
            .map_err(IpcError::from)?
    } else {
        let segment = app_state
            .store
            .find_segment(&input.segment_id)
            .map_err(IpcError::from)?
            .ok_or_else(|| store_error("E_SEGMENT_NOT_FOUND", "这条记录不存在".to_string()))?;
        let now = now_ms();
        let mut view = SegmentView::from_segment(&segment, now);
        if let Some(category) = category_view(&app_state, segment.category_id.as_deref())? {
            view = view.with_category(Some(category));
        }
        view
    };

    let now = now_ms();
    for tag in &input.tags {
        let name = tag.trim().trim_start_matches('#').trim();
        if name.is_empty() {
            continue;
        }
        app_state
            .store
            .segment_attach_tag(&input.segment_id, name, now)
            .map_err(IpcError::from)?;
    }
    Ok(view)
}

fn category_view(
    app_state: &AppState,
    category_id: Option<&str>,
) -> IpcResult<Option<CategoryView>> {
    let Some(id) = category_id else {
        return Ok(None);
    };
    let categories = app_state.timer.categories().map_err(IpcError::from)?;
    Ok(categories.into_iter().find(|category| category.id == id))
}

// ---------- 番茄Todo 集成命令 ----------

/// 读取番茄Todo 数据库并返回预览（不写入）。
#[tauri::command]
pub fn tomatodo_scan(state: tauri::State<'_, AppState>) -> IpcResult<ScanResult> {
    let settings = plugin_settings(&state, tomatodo::PLUGIN_ID);
    let db_path = settings
        .get(tomatodo::SETTING_DB_PATH)
        .and_then(|value| value.as_str());
    let path = resolve_db_path(db_path);
    scan(&path).map_err(|message| store_error("E_TOMATODO_SCAN", message))
}

/// 增量导入番茄Todo 专注记录为 DotLog 段。
#[tauri::command]
pub fn tomatodo_import(state: tauri::State<'_, AppState>) -> IpcResult<ImportResult> {
    let settings = plugin_settings(&state, tomatodo::PLUGIN_ID);
    let db_path = settings
        .get(tomatodo::SETTING_DB_PATH)
        .and_then(|value| value.as_str());
    let path = resolve_db_path(db_path);

    let last_imported_id = settings
        .get(tomatodo::SETTING_LAST_IMPORTED_ID)
        .and_then(|value| value.as_i64())
        .unwrap_or(0);

    let db = tomatodo::TomatodoDb::from_file(&path)
        .map_err(|message| store_error("E_TOMATODO_SCAN", message))?;

    let mut result = ImportResult::default();
    let mut max_imported_id = last_imported_id;

    for record in &db.pc_record {
        if !record.is_completed() {
            result.skipped_incomplete += 1;
            continue;
        }
        if record.id <= last_imported_id {
            result.skipped_existing += 1;
            continue;
        }
        let input = BackfillInput {
            start_at: record.start_date,
            end_at: record.end_at_ms(),
            title: Some(record.name.clone()),
            note: Some(format!("来自番茄Todo（记录 #{}）", record.id)),
            category_id: None,
            is_focus: true,
            is_ignored: false,
        };
        match state.timer.backfill_segment(input) {
            Ok(_) => {
                result.imported += 1;
                if record.id > max_imported_id {
                    max_imported_id = record.id;
                }
            }
            Err(error) => {
                // 时间重叠 → 冲突；其他 → 错误。
                let message = error.to_string();
                if message.contains("重叠") || message.contains("overlap") {
                    result.conflicts += 1;
                } else {
                    result.errors.push(format!("记录 #{}：{message}", record.id));
                }
            }
        }
    }

    // 更新游标。
    if max_imported_id > last_imported_id {
        let _ = state.timer.settings_patch(
            &format!("plugin:{}", tomatodo::PLUGIN_ID),
            vec![(
                tomatodo::SETTING_LAST_IMPORTED_ID.into(),
                serde_json::json!(max_imported_id),
            )],
        );
    }

    Ok(result)
}

/// 对比番茄Todo 记录与 DotLog 段（按时间重叠）。
#[tauri::command]
pub fn tomatodo_compare(state: tauri::State<'_, AppState>) -> IpcResult<CompareResult> {
    compare_internal(state.inner())
}

fn compare_internal(state: &AppState) -> IpcResult<CompareResult> {
    let settings = plugin_settings(state, tomatodo::PLUGIN_ID);
    let db_path = settings
        .get(tomatodo::SETTING_DB_PATH)
        .and_then(|value| value.as_str());
    let path = resolve_db_path(db_path);

    let db = tomatodo::TomatodoDb::from_file(&path)
        .map_err(|message| store_error("E_TOMATODO_SCAN", message))?;
    let completed: Vec<&tomatodo::FocusRecord> = db.completed_records();

    // 查询 DotLog 在番茄Todo 时间范围内的段。
    let segments: Vec<DotlogSegment> = if completed.is_empty() {
        Vec::new()
    } else {
        let min_start = completed.iter().map(|r| r.start_date).min().unwrap();
        let max_end = completed.iter().map(|r| r.end_at_ms()).max().unwrap();
        let dotlog_segments = state
            .store
            .day_segments(min_start, max_end)
            .map_err(IpcError::from)?;
        let categories = state.store.categories().unwrap_or_default();
        dotlog_segments
            .into_iter()
            .filter(|seg| seg.end_at_ms.is_some())
            .map(|seg| {
                let category_name = categories
                    .iter()
                    .find(|c| Some(c.id.as_str()) == seg.category_id.as_deref())
                    .map(|c| c.name.clone());
                DotlogSegment {
                    id: seg.id.to_string(),
                    start_at_ms: seg.start_at_ms,
                    end_at_ms: seg.end_at_ms.unwrap_or(seg.start_at_ms),
                    title: seg.title,
                    category_name,
                    tags: seg.tags,
                }
            })
            .collect()
    };

    let records: Vec<tomatodo::FocusRecord> = completed.into_iter().cloned().collect();
    Ok(compare(&records, &segments))
}

/// AI 分析：对比番茄Todo 粗粒度科目与 DotLog 细粒度活动，给出建议。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TomatodoAnalysis {
    pub pair_index: usize,
    pub tomatodo_subject: String,
    pub dotlog_summary: String,
    pub suggestion: String,
    pub reason: String,
}

#[tauri::command]
pub fn tomatodo_analyze(state: tauri::State<'_, AppState>) -> IpcResult<Vec<TomatodoAnalysis>> {
    if !state.host.plugin_enabled(dotlog_ai::PLUGIN_ID) {
        return Err(store_error("E_PLUGIN_DISABLED", "AI 助手未启用".to_string()));
    }
    let ai_settings = plugin_settings(&state, dotlog_ai::PLUGIN_ID);
    let privacy =
        PrivacyMode::from_setting(ai_settings.get("privacyMode").and_then(|v| v.as_str()));
    if privacy == PrivacyMode::Off {
        return Err(store_error("E_PLUGIN_DISABLED", "AI 联网已关闭（隐私模式 off）".to_string()));
    }

    // 先做对比。
    let compare_result = compare_internal(state.inner())?;

    // 只分析有 DotLog 段的配对。
    let analyzable: Vec<(usize, String, String)> = compare_result
        .pairs
        .iter()
        .enumerate()
        .filter_map(|(idx, pair)| {
            let tomatodo_name = pair.tomatodo.as_ref()?.name.clone();
            if pair.dotlog_segments.is_empty() {
                return None;
            }
            let dotlog_summary = pair
                .dotlog_segments
                .iter()
                .map(|seg| format!("{}（{}）", seg.title, seg.category_name.as_deref().unwrap_or("无分类")))
                .collect::<Vec<_>>()
                .join("；");
            Some((idx, tomatodo_name, dotlog_summary))
        })
        .collect();

    if analyzable.is_empty() {
        return Ok(Vec::new());
    }

    let provider = ai_provider(&ai_settings)?;
    let base = provider.base_url.clone();
    let client = UreqClient {
        allowed_hosts: host_of(&base).into_iter().collect(),
        timeout_ms: 20_000,
    };

    // 构建分析 prompt。
    let pairs_payload: Vec<serde_json::Value> = analyzable
        .iter()
        .map(|(_, subject, summary)| {
            serde_json::json!({
                "tomatodoSubject": subject,
                "dotlogSummary": summary,
            })
        })
        .collect();

    let system_prompt = "你是时间记录分析助手。用户同时使用番茄Todo（只记录科目+时长）和 DotLog（记录具体活动）。\
请分析每对数据：番茄Todo 记录的科目与 DotLog 记录的具体活动是否匹配？给出简短建议。\
只返回 JSON 数组，不要解释。元素格式：{\"tomatodoSubject\":\"...\",\"suggestion\":\"...\",\"reason\":\"一句话\"}。\
建议类型：1) 匹配良好；2) 科目与活动不符，建议修正；3) DotLog 记录过细，建议合并；4) DotLog 缺少细节，建议补充。";

    let body = serde_json::json!({
        "model": provider.model,
        "temperature": 0.3,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": serde_json::to_string(&pairs_payload).unwrap() }
        ],
    });

    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let Some(key) = &provider.api_key {
        headers.push(("Authorization".to_string(), format!("Bearer {key}")));
    }
    let url = format!("{}/chat/completions", provider.base_url);
    let request = dotlog_plugin_sdk::HttpRequest {
        method: "POST",
        url: &url,
        headers,
        body: Some(serde_json::to_vec(&body).map_err(|e| store_error("E_UNKNOWN", e.to_string()))?),
        timeout_ms: 20_000,
    };

    let response = client
        .send(request)
        .map_err(|e| store_error("E_NETWORK", e.to_string()))?;
    if !response.is_success() {
        return Err(store_error(
            "E_NETWORK",
            format!("AI 服务返回 HTTP {}", response.status),
        ));
    }

    // 解析 OpenAI 兼容响应：从 choices[0].message.content 取出模型文本，
    // 再剥掉可能的 markdown 代码块包裹，最后解析为 JSON 数组。
    let body = response.text();
    let content = parse_chat_content(&body);
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(&content).unwrap_or_else(|_| Vec::new());

    let results: Vec<TomatodoAnalysis> = parsed
        .into_iter()
        .zip(analyzable.iter())
        .map(|(value, (idx, subject, summary))| TomatodoAnalysis {
            pair_index: *idx,
            tomatodo_subject: subject.clone(),
            dotlog_summary: summary.clone(),
            suggestion: value
                .get("suggestion")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            reason: value
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    Ok(results)
}

/// 从 OpenAI 兼容的 `/chat/completions` 响应体中提取 `choices[0].message.content`。
/// 同时剥掉模型可能输出的 ```json ... ``` 代码块包裹。
fn parse_chat_content(body: &str) -> String {
    let extracted = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.to_string());

    // 去掉首尾空白与可能的 ```json / ``` 包裹。
    let trimmed = extracted.trim();
    if trimmed.starts_with("```") {
        let after_fence = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        let end = after_fence.rfind("```").unwrap_or(after_fence.len());
        after_fence[..end].trim().to_string()
    } else {
        trimmed.to_string()
    }
}
