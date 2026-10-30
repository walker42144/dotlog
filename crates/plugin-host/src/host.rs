//! 插件宿主（`docs/plugin-sdk.md` §5–§6）。
//!
//! - 启停：`setting(scope='plugin:<id>', key='enabled')`，默认停用（本地优先）。
//! - 裁决：能力来自 manifest；未声明的能力对应上下文对象为 `None`。
//! - 隔离：所有插件调用走独立线程 + `catch_unwind` + 超时；同一插件串行。
//! - outbox：统一退避重试与手动重试；handler 结论落到队列与回链表。

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use dotlog_application::{link_state, TimerStore};
use dotlog_domain::Segment;
use dotlog_plugin_sdk::{
    validate_manifest, Capability, HttpClient, IntegrationLinks, LinkRecord, OutboxEnv,
    OutboxHandler, OutboxInput, OutboxOutcome, OutboxSink, Plugin, PluginCtx, PluginError,
    PluginLogger, PluginManifest, PluginSettings, SecretStore, SegmentQuery, SegmentSnapshot,
    TagWriter,
};

use crate::net::{host_of, KeyringStore, UreqClient};

/// host 日志出口；desktop 注入每日文件日志实现，日志必须带插件 id（§12.4）。
pub trait HostLogger: Send + Sync {
    fn log(&self, level: &str, plugin_id: &str, message: &str);
}

/// 退避间隔（§6）：30 s → 2 min → 10 min → 1 h → 6 h。
pub const BACKOFF_MS: [i64; 5] = [30_000, 120_000, 600_000, 3_600_000, 21_600_000];
/// 最大尝试次数；达到即 `failed`，等待手动重试。
pub const MAX_ATTEMPTS: i64 = 6;
/// outbox 轮询间隔。
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);
const INIT_TIMEOUT: Duration = Duration::from_secs(3);
const EVENT_TIMEOUT: Duration = Duration::from_secs(3);
const HANDLER_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT_MS: u64 = 10_000;

struct Registered {
    plugin: Arc<dyn Plugin>,
    manifest: PluginManifest,
    /// 同一插件的事件串行执行（§5）。
    exec_lock: Arc<Mutex<()>>,
    /// init 失败的插件保持不可用，不接收事件。
    init_ok: bool,
}

pub struct PluginHost {
    store: Arc<dyn TimerStore>,
    logger: Arc<dyn HostLogger>,
    plugins: Mutex<Vec<Registered>>,
    handlers: Mutex<Vec<Arc<dyn OutboxHandler>>>,
    pub lifecycle: Mutex<()>,
}

impl PluginHost {
    pub fn new(store: Arc<dyn TimerStore>, logger: Arc<dyn HostLogger>) -> Self {
        Self {
            store,
            logger,
            plugins: Mutex::new(Vec::new()),
            handlers: Mutex::new(Vec::new()),
            lifecycle: Mutex::new(()),
        }
    }

    fn log(&self, level: &str, plugin_id: &str, message: &str) {
        self.logger.log(level, plugin_id, message);
    }

    /// 注册内置插件：校验 manifest → init（超时 3 s，失败禁用）。
    pub fn register(&self, plugin: Arc<dyn Plugin>) {
        let manifest = plugin.manifest();
        if let Err(error) = validate_manifest(&manifest) {
            self.log(
                "ERROR",
                &manifest.id,
                &format!("manifest 校验失败：{error}"),
            );
            return;
        }
        let id = manifest.id.clone();
        let init_id = id.clone();
        let init_ok = self
            .call_with_timeout(&id, "init", INIT_TIMEOUT, {
                let plugin = Arc::clone(&plugin);
                let store = Arc::clone(&self.store);
                let logger = Arc::clone(&self.logger);
                let capabilities = manifest.capabilities.clone();
                let network_hosts = manifest.network_hosts.clone();
                let entries = self.read_settings(&id);
                move || {
                    let kit = EnvKit::build(
                        store,
                        logger,
                        init_id,
                        capabilities,
                        network_hosts,
                        entries,
                        0,
                    );
                    let ctx = kit.ctx();
                    plugin.init(&ctx)
                }
            })
            .unwrap_or(Err(PluginError::Timeout));

        match init_ok {
            Ok(()) => {
                self.plugins.lock().unwrap().push(Registered {
                    plugin,
                    manifest,
                    exec_lock: Arc::new(Mutex::new(())),
                    init_ok: true,
                });
            }
            Err(error) => {
                self.log("ERROR", &id, &format!("init 失败，插件保持停用：{error}"));
            }
        }
    }

    pub fn register_handler(&self, handler: Arc<dyn OutboxHandler>) {
        self.handlers.lock().unwrap().push(handler);
    }

    pub fn manifests(&self) -> Vec<PluginManifest> {
        self.plugins
            .lock()
            .unwrap()
            .iter()
            .map(|registered| registered.manifest.clone())
            .collect()
    }

    fn scope_of(plugin_id: &str) -> String {
        format!("plugin:{plugin_id}")
    }

    /// 启用状态存 `plugin:<id>.enabled`；默认停用。
    pub fn plugin_installed(&self, plugin_id: &str) -> bool {
        self.read_settings(plugin_id)
            .into_iter()
            .find(|(key, _)| key == "installed")
            .and_then(|(_, value)| serde_json::from_str::<bool>(&value).ok())
            .unwrap_or(true)
    }

    pub fn plugin_enabled(&self, plugin_id: &str) -> bool {
        if !self.plugin_installed(plugin_id) {
            return false;
        }
        self.store
            .settings(&Self::scope_of(plugin_id))
            .ok()
            .and_then(|rows| {
                rows.into_iter()
                    .find(|(key, _)| key == "enabled")
                    .and_then(|(_, value)| serde_json::from_str::<bool>(&value).ok())
            })
            .unwrap_or(false)
    }

    pub fn set_enabled(&self, plugin_id: &str, enabled: bool, now_ms: i64) {
        let value = serde_json::to_string(&enabled).unwrap_or_else(|_| "false".into());
        let scope = Self::scope_of(plugin_id);
        if let Err(error) = self
            .store
            .settings_upsert(&scope, &[("enabled".into(), value)], now_ms)
        {
            self.log("ERROR", plugin_id, &format!("启停状态写入失败：{error}"));
        }
    }

    /// 段关闭事件分发：异步、逐插件隔离；不阻塞打点命令（§5）。
    pub fn dispatch_segment_closed(&self, event: &dotlog_plugin_sdk::SegmentClosedEvent) {
        struct Target {
            id: String,
            plugin: Arc<dyn Plugin>,
            exec_lock: Arc<Mutex<()>>,
            capabilities: Vec<Capability>,
            network_hosts: Vec<String>,
        }
        let targets: Vec<Target> = self
            .plugins
            .lock()
            .unwrap()
            .iter()
            .filter(|registered| registered.init_ok)
            .map(|registered| Target {
                id: registered.manifest.id.clone(),
                plugin: Arc::clone(&registered.plugin),
                exec_lock: Arc::clone(&registered.exec_lock),
                capabilities: registered.manifest.capabilities.clone(),
                network_hosts: registered.manifest.network_hosts.clone(),
            })
            .collect();

        for target in targets {
            if !self.plugin_enabled(&target.id) {
                continue;
            }
            let store = Arc::clone(&self.store);
            let logger = Arc::clone(&self.logger);
            let event = event.clone();
            std::thread::spawn(move || {
                // 串行锁在派生线程内持有：超时的孤儿线程继续占用，
                // 后续事件排队，避免同一插件乱序（§5）。
                let Target {
                    id,
                    plugin,
                    exec_lock,
                    capabilities,
                    network_hosts,
                } = target;

                let started = std::time::Instant::now();
                let (tx, rx) = mpsc::channel();
                let inner_id = id.clone();
                let inner_store = Arc::clone(&store);
                let inner_logger = Arc::clone(&logger);
                std::thread::spawn(move || {
                    let _serial = exec_lock.lock().unwrap_or_else(|e| e.into_inner());
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let kit = EnvKit::build(
                            Arc::clone(&inner_store),
                            Arc::clone(&inner_logger),
                            inner_id.clone(),
                            capabilities,
                            network_hosts,
                            PluginHostReader::settings_of(&inner_store, &inner_id),
                            event.segment.end_at_ms,
                        );
                        let ctx = kit.ctx();
                        plugin.on_segment_closed(&ctx, &event)
                    }));
                    let _ = tx.send(outcome);
                });
                match rx.recv_timeout(EVENT_TIMEOUT) {
                    Ok(Ok(Ok(()))) => {
                        let elapsed = started.elapsed().as_millis();
                        logger.log(
                            "INFO",
                            &id,
                            &format!("on_segment_closed 完成，耗时 {elapsed} ms"),
                        );
                    }
                    Ok(Ok(Err(error))) => {
                        logger.log("WARN", &id, &format!("on_segment_closed 失败：{error}"));
                    }
                    Ok(Err(panic)) => {
                        let message = panic
                            .downcast_ref::<&str>()
                            .map(|text| (*text).to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "未知 panic".into());
                        logger.log("ERROR", &id, &format!("on_segment_closed panic：{message}"));
                    }
                    Err(_) => {
                        logger.log("WARN", &id, "on_segment_closed 超时（3 s），已跳过");
                    }
                }
            });
        }
    }

    /// 处理一批到期事件；返回处理数量。desktop 的 worker 线程按
    /// [`TICK_INTERVAL`] 轮询，手动重试后立即调用一次。
    pub fn run_outbox_tick(&self, now_ms: i64) -> usize {
        let _lifecycle = self.lifecycle.lock().unwrap_or_else(|e| e.into_inner());
        let due = match self.store.outbox_take_due(now_ms, 20) {
            Ok(due) => due,
            Err(error) => {
                self.log("ERROR", "outbox", &format!("取出到期事件失败：{error}"));
                return 0;
            }
        };
        if due.is_empty() {
            return 0;
        }
        let handlers: Vec<Arc<dyn OutboxHandler>> = self.handlers.lock().unwrap().clone();
        let mut processed = 0;

        for record in due {
            processed += 1;
            let payload: serde_json::Value =
                serde_json::from_str(&record.payload).unwrap_or(serde_json::Value::Null);
            let provider = record
                .event_type
                .split('.')
                .next()
                .unwrap_or("unknown")
                .to_string();

            let Some(handler) = handlers
                .iter()
                .find(|handler| handler.event_type() == record.event_type)
            else {
                let _ = self.store.outbox_mark_retry(
                    &record.id,
                    "没有可用的处理器，6 小时后重试",
                    now_ms + 6 * 3_600_000,
                    now_ms,
                );
                continue;
            };

            if record.event_type == "memos.publish" && now_ms < record.created_at_ms + 65_000 {
                let _ = self
                    .store
                    .outbox_defer(&record.id, record.created_at_ms + 65_000, now_ms);
                continue;
            }
            if record.event_type == "memos.publish" {
                if let Some(id) = payload.get("segmentId").and_then(|v| v.as_str()) {
                    match self.store.publication_ready(id) {
                        Ok(true) => {}
                        _ => {
                            let _ = self.store.outbox_defer(&record.id, now_ms + 30_000, now_ms);
                            continue;
                        }
                    }
                }
            }
            let plugin_id = handler.plugin_id().to_string();
            if !self.plugin_enabled(&plugin_id) {
                let _ = self.store.outbox_defer(&record.id, now_ms + 30_000, now_ms);
                continue;
            }
            let (capabilities, network_hosts) = self
                .manifest_of(&plugin_id)
                .map(|manifest| (manifest.capabilities, manifest.network_hosts))
                .unwrap_or_default();
            let entries = self.read_settings(&plugin_id);
            let handle_plugin = plugin_id.clone();
            let handle_payload = payload.clone();
            let outcome = self
                .call_with_timeout(&plugin_id, "outbox.handle", HANDLER_TIMEOUT, {
                    let handler = Arc::clone(handler);
                    let store = Arc::clone(&self.store);
                    let logger = Arc::clone(&self.logger);
                    move || {
                        let kit = EnvKit::build(
                            store,
                            logger,
                            handle_plugin,
                            capabilities,
                            network_hosts,
                            entries,
                            now_ms,
                        );
                        let env = kit.outbox_env();
                        handler.handle(&handle_payload, &env)
                    }
                })
                .unwrap_or(OutboxOutcome::Fail {
                    error: "处理器超时（10 s）".into(),
                });

            match outcome {
                OutboxOutcome::Done { external_id } => {
                    let _ = self.store.outbox_mark_done(&record.id, now_ms);
                    self.update_link(
                        &payload,
                        &provider,
                        external_id.as_deref(),
                        link_state::PUBLISHED,
                        None,
                        now_ms,
                    );
                    self.log("INFO", &plugin_id, &format!("事件 {} 处理完成", record.id));
                }
                OutboxOutcome::RetryLater { error } => {
                    let index = (record.attempts.max(0) as usize).min(BACKOFF_MS.len() - 1);
                    let delay = BACKOFF_MS[index];
                    let _ =
                        self.store
                            .outbox_mark_retry(&record.id, &error, now_ms + delay, now_ms);
                    self.log(
                        "WARN",
                        &plugin_id,
                        &format!(
                            "事件 {} 第 {} 次失败，{} s 后重试：{error}",
                            record.id,
                            record.attempts + 1,
                            delay / 1_000
                        ),
                    );
                }
                OutboxOutcome::Fail { error } => {
                    let _ = self.store.outbox_mark_failed(&record.id, &error, now_ms);
                    self.update_link(
                        &payload,
                        &provider,
                        None,
                        link_state::FAILED,
                        Some(&error),
                        now_ms,
                    );
                    self.log(
                        "ERROR",
                        &plugin_id,
                        &format!("事件 {} 处理失败：{error}", record.id),
                    );
                }
            }
        }
        processed
    }

    fn update_link(
        &self,
        payload: &serde_json::Value,
        provider: &str,
        external_id: Option<&str>,
        state: &str,
        error: Option<&str>,
        now_ms: i64,
    ) {
        let Some(segment_id) = payload.get("segmentId").and_then(|v| v.as_str()) else {
            return;
        };
        let _ = self.store.integration_link_upsert(
            segment_id,
            provider,
            external_id,
            state,
            error,
            now_ms,
        );
    }

    /// 在独立线程执行 `f`（panic 隔离 + 超时）；超时/panic 返回 `None`。
    fn call_with_timeout<T: Send + 'static>(
        &self,
        plugin_id: &str,
        label: &str,
        timeout: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(outcome);
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(value)) => Some(value),
            Ok(Err(panic)) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "未知 panic".into());
                self.log("ERROR", plugin_id, &format!("{label} panic：{message}"));
                None
            }
            Err(_) => {
                self.log(
                    "WARN",
                    plugin_id,
                    &format!("{label} 超时（{} ms）", timeout.as_millis()),
                );
                None
            }
        }
    }

    fn manifest_of(&self, plugin_id: &str) -> Option<PluginManifest> {
        self.plugins
            .lock()
            .unwrap()
            .iter()
            .find(|registered| registered.manifest.id == plugin_id)
            .map(|registered| registered.manifest.clone())
    }

    fn read_settings(&self, plugin_id: &str) -> Vec<(String, String)> {
        self.store
            .settings(&Self::scope_of(plugin_id))
            .unwrap_or_default()
    }
}

/// 注册表只读访问（dispatch 闭包内读取插件设置）。
struct PluginHostReader;

impl PluginHostReader {
    fn settings_of(store: &Arc<dyn TimerStore>, plugin_id: &str) -> Vec<(String, String)> {
        store
            .settings(&format!("plugin:{plugin_id}"))
            .unwrap_or_default()
    }
}

/// 一次插件调用 / outbox 处理所需的全部能力对象（owned，可跨线程）。
struct EnvKit {
    plugin_id: String,
    outbox_sink: StoreOutboxSink,
    settings: StoreSettings,
    links: StoreLinks,
    log_bridge: PluginLogBridge,
    http: Option<UreqClient>,
    secrets: Option<KeyringStore>,
    segments: Option<StoreSegmentQuery>,
    tags: Option<StoreTagWriter>,
    now_ms: i64,
}

impl EnvKit {
    fn build(
        store: Arc<dyn TimerStore>,
        logger: Arc<dyn HostLogger>,
        plugin_id: String,
        capabilities: Vec<Capability>,
        network_hosts: Vec<String>,
        settings_entries: Vec<(String, String)>,
        now_ms: i64,
    ) -> Self {
        let has = |cap: &Capability| capabilities.contains(cap);
        // 动态白名单：manifest 声明 + 设置中的实例地址（Memos instanceUrl / AI baseUrl）。
        let mut allowed_hosts = network_hosts;
        let instance_host = ["instanceUrl", "baseUrl"].iter().find_map(|key| {
            settings_entries
                .iter()
                .find(|(name, _)| name == key)
                .and_then(|(_, raw)| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|value| value.as_str().and_then(host_of))
        });
        if let Some(host) = instance_host {
            allowed_hosts.push(host);
        }
        Self {
            outbox_sink: StoreOutboxSink {
                store: Arc::clone(&store),
                now_ms,
            },
            settings: StoreSettings {
                entries: settings_entries,
            },
            links: StoreLinks {
                store: Arc::clone(&store),
            },
            log_bridge: PluginLogBridge {
                logger: Arc::clone(&logger),
                plugin_id: plugin_id.clone(),
            },
            http: has(&Capability::Network).then_some(UreqClient {
                allowed_hosts,
                timeout_ms: HTTP_TIMEOUT_MS,
            }),
            secrets: has(&Capability::SecretStore).then(|| KeyringStore::for_plugin(&plugin_id)),
            segments: has(&Capability::SegmentsRead).then(|| StoreSegmentQuery {
                store: Arc::clone(&store),
            }),
            tags: has(&Capability::TagsWrite).then(|| StoreTagWriter {
                store: Arc::clone(&store),
            }),
            plugin_id,
            now_ms,
        }
    }

    /// 插件调用上下文：未声明的能力为 `None`（§4）。
    fn ctx(&self) -> PluginCtx<'_> {
        PluginCtx {
            plugin_id: &self.plugin_id,
            segments: self.segments.as_ref().map(|q| q as &dyn SegmentQuery),
            tags: self.tags.as_ref().map(|t| t as &dyn TagWriter),
            http: self.http.as_ref().map(|h| h as &dyn HttpClient),
            secrets: self.secrets.as_ref().map(|s| s as &dyn SecretStore),
            outbox: &self.outbox_sink,
            settings: &self.settings,
            links: &self.links,
            log: &self.log_bridge,
            now_ms: self.now_ms,
        }
    }

    /// outbox handler 运行环境：同样按能力裁决。
    fn outbox_env(&self) -> OutboxEnv<'_> {
        OutboxEnv {
            segments: self.segments.as_ref().map(|q| q as &dyn SegmentQuery),
            http: self.http.as_ref().map(|h| h as &dyn HttpClient),
            secrets: self.secrets.as_ref().map(|s| s as &dyn SecretStore),
            settings: &self.settings,
            links: &self.links,
            log: &self.log_bridge,
            now_ms: self.now_ms,
        }
    }
}

// ---------- TimerStore 桥接适配器 ----------

struct StoreOutboxSink {
    store: Arc<dyn TimerStore>,
    now_ms: i64,
}

impl OutboxSink for StoreOutboxSink {
    fn enqueue(&self, event: OutboxInput) -> Result<(), PluginError> {
        let payload = serde_json::to_string(&event.payload)
            .map_err(|error| PluginError::Other(error.to_string()))?;
        self.store
            .outbox_enqueue(
                Some(&event.dedupe_key),
                &event.event_type,
                &payload,
                self.now_ms,
            )
            // 去重键已存在（返回 false）视为幂等成功（§6）。
            .map(|_enqueued| ())
            .map_err(|error| PluginError::Storage(error.to_string()))
    }
}

struct StoreSettings {
    entries: Vec<(String, String)>,
}

impl PluginSettings for StoreSettings {
    fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, raw)| serde_json::from_str(raw).ok())
    }
}

struct StoreLinks {
    store: Arc<dyn TimerStore>,
}

impl IntegrationLinks for StoreLinks {
    fn get(&self, segment_id: &str, provider: &str) -> Result<Option<LinkRecord>, PluginError> {
        self.store
            .integration_link_get(segment_id, provider)
            .map(|row| {
                row.map(|row| LinkRecord {
                    segment_id: row.segment_id,
                    provider: row.provider,
                    external_id: row.external_id,
                    state: row.state,
                    last_synced_at_ms: row.last_synced_at_ms,
                    last_error: row.last_error,
                })
            })
            .map_err(|error| PluginError::Storage(error.to_string()))
    }

    fn mark_published(
        &self,
        segment_id: &str,
        provider: &str,
        external_id: &str,
        now_ms: i64,
    ) -> Result<(), PluginError> {
        self.store
            .integration_link_upsert(
                segment_id,
                provider,
                Some(external_id),
                link_state::PUBLISHED,
                None,
                now_ms,
            )
            .map_err(|error| PluginError::Storage(error.to_string()))
    }

    fn mark_failed(
        &self,
        segment_id: &str,
        provider: &str,
        error: &str,
        now_ms: i64,
    ) -> Result<(), PluginError> {
        self.store
            .integration_link_upsert(
                segment_id,
                provider,
                None,
                link_state::FAILED,
                Some(error),
                now_ms,
            )
            .map_err(|error| PluginError::Storage(error.to_string()))
    }
}

struct PluginLogBridge {
    logger: Arc<dyn HostLogger>,
    plugin_id: String,
}

impl PluginLogger for PluginLogBridge {
    fn info(&self, message: &str) {
        self.logger.log("INFO", &self.plugin_id, message);
    }

    fn warn(&self, message: &str) {
        self.logger.log("WARN", &self.plugin_id, message);
    }

    fn error(&self, message: &str) {
        self.logger.log("ERROR", &self.plugin_id, message);
    }
}

struct StoreSegmentQuery {
    store: Arc<dyn TimerStore>,
}

impl SegmentQuery for StoreSegmentQuery {
    fn segment_by_id(&self, id: &str) -> Result<Option<SegmentSnapshot>, PluginError> {
        let segment = self
            .store
            .find_segment(id)
            .map_err(|e| PluginError::Storage(e.to_string()))?;
        let categories = self
            .store
            .categories()
            .map_err(|e| PluginError::Storage(e.to_string()))?;
        let Some(segment) = segment else {
            return Ok(None);
        };
        if self
            .store
            .current_session_segments()
            .map_err(|e| PluginError::Storage(e.to_string()))?
            .iter()
            .any(|s| s.id == id)
        {
            return Ok(None);
        }
        let slices = self
            .store
            .session_segments(id)
            .map_err(|e| PluginError::Storage(e.to_string()))?;
        if slices.iter().any(|s| s.is_active()) {
            return Ok(None);
        }
        let mut snapshot = segment_snapshot(&segment, &categories);
        if let Some(first) = slices.first() {
            snapshot.start_at_ms = first.start_at_ms;
        }
        snapshot.end_at_ms = slices.iter().filter_map(|s| s.end_at_ms).max().unwrap_or(snapshot.end_at_ms);
        snapshot.duration_seconds = slices
            .iter()
            .map(|s| s.duration_ms(s.end_at_ms.unwrap_or(s.start_at_ms)))
            .sum::<i64>()
            / 1000;
        Ok(Some(snapshot))
    }
    fn segments_between(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<SegmentSnapshot>, PluginError> {
        let segments = self
            .store
            .day_segments(from_ms, to_ms)
            .map_err(|error| PluginError::Storage(error.to_string()))?;
        let categories = self.store.categories().unwrap_or_default();
        Ok(segments
            .iter()
            .map(|segment| segment_snapshot(segment, &categories))
            .collect())
    }
}

struct StoreTagWriter {
    store: Arc<dyn TimerStore>,
}

impl TagWriter for StoreTagWriter {
    fn add_tag(&self, segment_id: &str, tag: &str) -> Result<(), PluginError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as i64)
            .unwrap_or(0);
        self.store
            .segment_attach_tag(segment_id, tag, now_ms)
            .map_err(|error| PluginError::Storage(error.to_string()))
    }
}

/// 领域段 → 不可变快照。
pub fn segment_snapshot(
    segment: &Segment,
    categories: &[dotlog_application::CategoryRow],
) -> SegmentSnapshot {
    let end = segment.end_at_ms.unwrap_or(segment.start_at_ms);
    let category_name = segment
        .category_id
        .as_deref()
        .and_then(|id| categories.iter().find(|row| row.id == id))
        .map(|row| row.name.clone());
    SegmentSnapshot {
        id: segment.id.clone(),
        start_at_ms: segment.start_at_ms,
        end_at_ms: end,
        duration_seconds: (end - segment.start_at_ms) / 1_000,
        title: segment.title.clone(),
        note: segment.note.clone(),
        tags: segment.tags.clone(),
        category_id: segment.category_id.clone(),
        category_name,
        is_focus: segment.is_focus,
    }
}
