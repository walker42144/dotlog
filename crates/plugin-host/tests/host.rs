//! 插件宿主集成测试：启停、事件分发、panic 隔离、能力裁决与 outbox 退避。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dotlog_application::{
    AppResult, CategoryRow, IntegrationLinkRow, OutboxRecord, PointRecord, TimerSnapshot,
    TimerStore, link_state, outbox_state,
};
use dotlog_domain::{Effect, Segment};
use dotlog_plugin_host::host::{BACKOFF_MS, HostLogger, PluginHost};
use dotlog_plugin_sdk::{
    API_VERSION, Capability, CloseReason, OutboxEnv, OutboxHandler, OutboxInput, OutboxOutcome,
    Plugin, PluginCtx, PluginError, PluginManifest, SegmentClosedEvent, SegmentSnapshot,
};
use serde_json::json;

const NOW: i64 = 1_700_000_000_000;

// ---------- Mock 基础设施 ----------

struct MockStore {
    inner: Mutex<MockData>,
}

#[derive(Default)]
struct MockData {
    segments: Vec<Segment>,
    settings: BTreeMap<String, Vec<(String, String)>>,
    outbox: Vec<OutboxRecord>,
    links: Vec<IntegrationLinkRow>,
    next_outbox_id: u64,
}

impl MockStore {
    fn new() -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(MockData::default()) })
    }

    fn outbox_first(&self) -> Option<OutboxRecord> {
        self.inner.lock().unwrap().outbox.first().cloned()
    }

    fn link(&self, segment_id: &str, provider: &str) -> Option<IntegrationLinkRow> {
        self.inner
            .lock()
            .unwrap()
            .links
            .iter()
            .find(|row| row.segment_id == segment_id && row.provider == provider)
            .cloned()
    }
}

impl TimerStore for MockStore {
    fn snapshot(&self) -> AppResult<TimerSnapshot> {
        let data = self.inner.lock().unwrap();
        Ok(TimerSnapshot {
            active: data.segments.iter().find(|s| s.end_at_ms.is_none()).cloned(),
            last_point_kind: None,
        })
    }

    fn commit(&self, effects: &[Effect], _now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for effect in effects {
            if let Effect::InsertSegment(segment) = effect {
                data.segments.push(segment.clone());
            }
        }
        Ok(())
    }

    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>> {
        let data = self.inner.lock().unwrap();
        let mut items: Vec<Segment> = data
            .segments
            .iter()
            .filter(|s| {
                s.start_at_ms < to_ms
                    && match s.end_at_ms {
                        Some(end) => end > from_ms,
                        None => true,
                    }
            })
            .cloned()
            .collect();
        items.sort_by_key(|s| s.start_at_ms);
        Ok(items)
    }

    fn find_segment(&self, id: &str) -> AppResult<Option<Segment>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .segments
            .iter()
            .find(|s| s.id == id)
            .cloned())
    }

    fn last_point(&self) -> AppResult<Option<PointRecord>> {
        Ok(None)
    }

    fn history_segments(
        &self,
        from_ms: i64,
        to_ms: i64,
        _text: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)> {
        let mut items = self.day_segments(from_ms, to_ms)?;
        items.reverse();
        let total = items.len() as u64;
        let page = items
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        Ok((page, total))
    }

    fn categories(&self) -> AppResult<Vec<CategoryRow>> {
        Ok(Vec::new())
    }

    fn settings(&self, scope: &str) -> AppResult<Vec<(String, String)>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .settings
            .get(scope)
            .cloned()
            .unwrap_or_default())
    }

    fn settings_upsert(
        &self,
        scope: &str,
        entries: &[(String, String)],
        _now_ms: i64,
    ) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        let row = data.settings.entry(scope.to_string()).or_default();
        for (key, value) in entries {
            match row.iter_mut().find(|(name, _)| name == key) {
                Some(existing) => existing.1 = value.clone(),
                None => row.push((key.clone(), value.clone())),
            }
        }
        Ok(())
    }

    fn outbox_enqueue(
        &self,
        dedupe_key: Option<&str>,
        event_type: &str,
        payload: &str,
        now_ms: i64,
    ) -> AppResult<bool> {
        let mut data = self.inner.lock().unwrap();
        if let Some(key) = dedupe_key {
            if data.outbox.iter().any(|r| r.dedupe_key.as_deref() == Some(key)) {
                return Ok(false);
            }
        }
        data.next_outbox_id += 1;
        let id = format!("evt-{}", data.next_outbox_id);
        data.outbox.push(OutboxRecord {
            id,
            dedupe_key: dedupe_key.map(str::to_string),
            event_type: event_type.to_string(),
            payload: payload.to_string(),
            state: outbox_state::PENDING.into(),
            attempts: 0,
            next_retry_at_ms: None,
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        });
        Ok(true)
    }

    fn outbox_take_due(&self, now_ms: i64, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        let mut data = self.inner.lock().unwrap();
        let mut taken = Vec::new();
        for record in data.outbox.iter_mut() {
            if taken.len() >= limit as usize {
                break;
            }
            let due = if record.state == outbox_state::PENDING {
                match record.next_retry_at_ms {
                    Some(t) => t <= now_ms,
                    None => true,
                }
            } else if record.state == outbox_state::INFLIGHT {
                // 租约内的 inflight 不可再取，到期后才回收。
                record.next_retry_at_ms.is_some_and(|t| t <= now_ms)
            } else {
                false
            };
            if due {
                record.state = outbox_state::INFLIGHT.into();
                record.next_retry_at_ms = Some(now_ms + 300_000);
                record.updated_at_ms = now_ms;
                taken.push(record.clone());
            }
        }
        Ok(taken)
    }

    fn outbox_mark_done(&self, id: &str, now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        if let Some(record) = data.outbox.iter_mut().find(|r| r.id == id) {
            record.state = outbox_state::DONE.into();
            record.next_retry_at_ms = None;
            record.updated_at_ms = now_ms;
        }
        Ok(())
    }

    fn outbox_mark_retry(
        &self,
        id: &str,
        error: &str,
        next_retry_at_ms: i64,
        now_ms: i64,
    ) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        if let Some(record) = data.outbox.iter_mut().find(|r| r.id == id) {
            record.attempts += 1;
            if record.attempts >= 6 {
                record.state = outbox_state::FAILED.into();
                record.next_retry_at_ms = None;
            } else {
                record.state = outbox_state::PENDING.into();
                record.next_retry_at_ms = Some(next_retry_at_ms);
            }
            record.last_error = Some(error.to_string());
            record.updated_at_ms = now_ms;
        }
        Ok(())
    }

    fn outbox_mark_failed(&self, id: &str, error: &str, now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        if let Some(record) = data.outbox.iter_mut().find(|r| r.id == id) {
            record.state = outbox_state::FAILED.into();
            record.next_retry_at_ms = None;
            record.last_error = Some(error.to_string());
            record.updated_at_ms = now_ms;
        }
        Ok(())
    }

    fn outbox_list(&self, state: Option<&str>, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        let data = self.inner.lock().unwrap();
        let mut items: Vec<OutboxRecord> = data
            .outbox
            .iter()
            .filter(|r| match state {
                Some(s) => r.state == s,
                None => true,
            })
            .cloned()
            .collect();
        items.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
        items.truncate(limit as usize);
        Ok(items)
    }

    fn outbox_retry(&self, id: &str, now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        if let Some(record) = data.outbox.iter_mut().find(|r| r.id == id) {
            if record.state != outbox_state::DONE {
                record.state = outbox_state::PENDING.into();
                record.attempts = 0;
                record.next_retry_at_ms = None;
                record.updated_at_ms = now_ms;
            }
        }
        Ok(())
    }

    fn outbox_counts(&self) -> AppResult<Vec<(String, i64)>> {
        let data = self.inner.lock().unwrap();
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for record in &data.outbox {
            *counts.entry(record.state.clone()).or_insert(0) += 1;
        }
        Ok(counts.into_iter().collect())
    }

    fn integration_link_get(
        &self,
        segment_id: &str,
        provider: &str,
    ) -> AppResult<Option<IntegrationLinkRow>> {
        Ok(self.link(segment_id, provider))
    }

    fn integration_link_upsert(
        &self,
        segment_id: &str,
        provider: &str,
        external_id: Option<&str>,
        state: &str,
        error: Option<&str>,
        now_ms: i64,
    ) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        let row = match data
            .links
            .iter_mut()
            .find(|row| row.segment_id == segment_id && row.provider == provider)
        {
            Some(row) => row,
            None => {
                data.links.push(IntegrationLinkRow {
                    segment_id: segment_id.to_string(),
                    provider: provider.to_string(),
                    external_id: None,
                    state: state.to_string(),
                    last_synced_at_ms: None,
                    last_error: None,
                });
                data.links.last_mut().unwrap()
            }
        };
        row.external_id = external_id.map(str::to_string);
        row.state = state.to_string();
        row.last_error = error.map(str::to_string);
        row.last_synced_at_ms = (state == link_state::PUBLISHED).then_some(now_ms);
        Ok(())
    }
}

#[derive(Default)]
struct MockLogger {
    entries: Mutex<Vec<(String, String, String)>>,
}

impl HostLogger for MockLogger {
    fn log(&self, level: &str, plugin_id: &str, message: &str) {
        self.entries
            .lock()
            .unwrap()
            .push((level.to_string(), plugin_id.to_string(), message.to_string()));
    }
}

// ---------- 测试插件与处理器 ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 只记录收到的事件。
    Record,
    /// 记录事件并向 outbox 投递（先入队后记录，便于用事件数判断入队完成）。
    Enqueue,
    /// 在 on_segment_closed 中 panic。
    Panic,
    /// 记录能力裁决结果。
    InspectCaps,
}

#[derive(Debug, Clone, PartialEq)]
struct CapsView {
    http: bool,
    secrets: bool,
    segments: bool,
    tags: bool,
    instance_url: Option<String>,
}

struct TestPlugin {
    manifest: PluginManifest,
    mode: Mode,
    events: Mutex<Vec<SegmentClosedEvent>>,
    calls: AtomicUsize,
    caps: Mutex<Option<CapsView>>,
}

impl TestPlugin {
    fn new(id: &str, capabilities: Vec<Capability>, mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            manifest: manifest(id, capabilities),
            mode,
            events: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            caps: Mutex::new(None),
        })
    }

    fn event_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    fn caps_view(&self) -> Option<CapsView> {
        self.caps.lock().unwrap().clone()
    }
}

impl Plugin for TestPlugin {
    fn manifest(&self) -> PluginManifest {
        self.manifest.clone()
    }

    fn on_segment_closed(
        &self,
        ctx: &PluginCtx,
        event: &SegmentClosedEvent,
    ) -> Result<(), PluginError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::Record => self.events.lock().unwrap().push(event.clone()),
            Mode::Enqueue => {
                ctx.outbox.enqueue(OutboxInput {
                    dedupe_key: format!("test:{}:publish", event.segment.id),
                    event_type: "test.publish".into(),
                    payload: json!({ "segmentId": event.segment.id, "title": event.segment.title }),
                })?;
                self.events.lock().unwrap().push(event.clone());
            }
            Mode::Panic => panic!("boom"),
            Mode::InspectCaps => {
                *self.caps.lock().unwrap() = Some(CapsView {
                    http: ctx.http.is_some(),
                    secrets: ctx.secrets.is_some(),
                    segments: ctx.segments.is_some(),
                    tags: ctx.tags.is_some(),
                    instance_url: ctx
                        .settings
                        .get("instanceUrl")
                        .and_then(|value| value.as_str().map(str::to_string)),
                });
            }
        }
        Ok(())
    }
}

struct TestHandler {
    plugin_id: String,
    event_type: &'static str,
    outcome: Arc<Mutex<OutboxOutcome>>,
}

impl OutboxHandler for TestHandler {
    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    fn event_type(&self) -> &'static str {
        self.event_type
    }

    fn handle(&self, _payload: &serde_json::Value, _env: &OutboxEnv) -> OutboxOutcome {
        self.outcome.lock().unwrap().clone()
    }
}

// ---------- 辅助 ----------

fn manifest(id: &str, capabilities: Vec<Capability>) -> PluginManifest {
    PluginManifest {
        id: id.to_string(),
        name: "测试插件".into(),
        version: "0.1.0".into(),
        api_version: API_VERSION,
        capabilities,
        network_hosts: vec![],
        settings_schema: serde_json::Value::Null,
        entry: format!("builtin:{id}"),
    }
}

fn event(segment_id: &str) -> SegmentClosedEvent {
    SegmentClosedEvent {
        segment: SegmentSnapshot {
            id: segment_id.to_string(),
            start_at_ms: NOW,
            end_at_ms: NOW + 60_000,
            duration_seconds: 60,
            title: "写代码".into(),
            note: String::new(),
            tags: vec![],
            category_id: None,
            category_name: None,
            is_focus: false,
        },
        reason: CloseReason::Punch,
    }
}

fn wait_for(predicate: impl Fn() -> bool, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    predicate()
}

// ---------- 测试 ----------

#[test]
fn disabled_plugin_does_not_receive_events() {
    let store = MockStore::new();
    let host = PluginHost::new(store, Arc::new(MockLogger::default()));
    let plugin = TestPlugin::new("disabled-plugin", vec![], Mode::Record);
    host.register(plugin.clone());

    assert!(!host.plugin_enabled("disabled-plugin"), "默认应停用");
    host.dispatch_segment_closed(&event("seg-1"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(plugin.event_count(), 0, "停用的插件不应收到事件");
}

#[test]
fn event_flow_enqueues_outbox_and_publishes_link_idempotently() {
    let store = MockStore::new();
    let host = PluginHost::new(store.clone(), Arc::new(MockLogger::default()));
    let plugin = TestPlugin::new("publisher", vec![Capability::SegmentsRead], Mode::Enqueue);
    host.register(plugin.clone());
    host.set_enabled("publisher", true, NOW);
    assert!(host.plugin_enabled("publisher"));

    host.dispatch_segment_closed(&event("seg-1"));
    assert!(wait_for(|| plugin.event_count() == 1, 2_000), "启用的插件应收到事件");
    assert_eq!(store.outbox_list(None, 100).unwrap().len(), 1, "应入队一条事件");

    let outcome = Arc::new(Mutex::new(OutboxOutcome::Done {
        external_id: Some("memo-1".into()),
    }));
    host.register_handler(Arc::new(TestHandler {
        plugin_id: "publisher".into(),
        event_type: "test.publish",
        outcome,
    }));

    assert_eq!(host.run_outbox_tick(NOW + 1_000), 1);
    assert_eq!(
        store.outbox_counts().unwrap(),
        vec![("done".to_string(), 1)]
    );
    let link = store.link("seg-1", "test").expect("应写入回链");
    assert_eq!(link.state, link_state::PUBLISHED);
    assert_eq!(link.external_id.as_deref(), Some("memo-1"));

    // 同一事件再次分发：去重键相同，不产生新事件。
    host.dispatch_segment_closed(&event("seg-1"));
    assert!(wait_for(|| plugin.event_count() == 2, 2_000));
    assert_eq!(store.outbox_list(None, 100).unwrap().len(), 1, "去重键应阻止重复入队");
    assert_eq!(host.run_outbox_tick(NOW + 2_000), 0, "已完成的事件不应重复处理");
}

#[test]
fn panicking_plugin_does_not_break_host() {
    let store = MockStore::new();
    let logger = Arc::new(MockLogger::default());
    let host = PluginHost::new(store, logger.clone());
    let crasher = TestPlugin::new("crasher", vec![], Mode::Panic);
    host.register(crasher.clone());
    host.set_enabled("crasher", true, NOW);
    let recorder = TestPlugin::new("recorder", vec![], Mode::Record);
    host.register(recorder.clone());
    host.set_enabled("recorder", true, NOW);

    host.dispatch_segment_closed(&event("seg-1"));
    assert!(
        wait_for(
            || logger
                .entries
                .lock()
                .unwrap()
                .iter()
                .any(|(level, id, message)| level == "ERROR"
                    && id == "crasher"
                    && message.contains("panic")),
            2_000
        ),
        "panic 应被 host 捕获并记录"
    );
    assert_eq!(crasher.calls.load(Ordering::SeqCst), 1);

    // host 仍正常分发后续事件给其它插件。
    host.dispatch_segment_closed(&event("seg-2"));
    assert!(wait_for(|| recorder.event_count() == 1, 2_000), "其它插件应照常收到事件");
}

#[test]
fn undeclared_capabilities_are_not_injected() {
    let store = MockStore::new();
    store
        .settings_upsert(
            "plugin:inspector",
            &[("instanceUrl".into(), serde_json::to_string("https://memos.local").unwrap())],
            NOW,
        )
        .unwrap();
    let host = PluginHost::new(store, Arc::new(MockLogger::default()));
    let plugin = TestPlugin::new("inspector", vec![], Mode::InspectCaps);
    host.register(plugin.clone());
    host.set_enabled("inspector", true, NOW);

    host.dispatch_segment_closed(&event("seg-1"));
    assert!(wait_for(|| plugin.caps_view().is_some(), 2_000));
    let caps = plugin.caps_view().unwrap();
    assert!(!caps.http, "未声明 network，http 应为 None");
    assert!(!caps.secrets, "未声明 secret:store，secrets 应为 None");
    assert!(!caps.segments, "未声明 segments:read，segments 应为 None");
    assert!(!caps.tags, "未声明 tags:write，tags 应为 None");
    assert_eq!(
        caps.instance_url.as_deref(),
        Some("https://memos.local"),
        "settings 应可读且 JSON 解析生效"
    );
}

#[test]
fn outbox_backoff_manual_retry_and_recovery() {
    let store = MockStore::new();
    let host = PluginHost::new(store.clone(), Arc::new(MockLogger::default()));
    let outcome = Arc::new(Mutex::new(OutboxOutcome::RetryLater {
        error: "网络不可达".into(),
    }));
    host.register_handler(Arc::new(TestHandler {
        plugin_id: "publisher".into(),
        event_type: "test.publish",
        outcome: outcome.clone(),
    }));

    store
        .outbox_enqueue(Some("k1"), "test.publish", r#"{"segmentId":"seg-1"}"#, NOW)
        .unwrap();
    let record_id = store.outbox_list(None, 10).unwrap()[0].id.clone();

    // 退避表：30 s → 2 min → 10 min → 1 h → 6 h，第 6 次失败转 failed。
    let delays = [
        BACKOFF_MS[0],
        BACKOFF_MS[1],
        BACKOFF_MS[2],
        BACKOFF_MS[3],
        BACKOFF_MS[4],
        BACKOFF_MS[4],
    ];
    let mut now = NOW;
    for (round, delay) in delays.iter().enumerate() {
        assert_eq!(host.run_outbox_tick(now), 1, "第 {} 轮应处理到期事件", round + 1);
        assert_eq!(host.run_outbox_tick(now), 0, "退避期内不应重复取出");
        let record = store.outbox_first().unwrap();
        if round == delays.len() - 1 {
            assert_eq!(record.state, outbox_state::FAILED, "第 6 次失败应转 failed");
            assert_eq!(record.attempts, 6);
            assert_eq!(record.next_retry_at_ms, None);
        } else {
            assert_eq!(record.state, outbox_state::PENDING);
            assert_eq!(record.attempts, round as i64 + 1);
            assert_eq!(record.next_retry_at_ms, Some(now + delay));
        }
        now += delay;
    }

    // 手动重试：回到 pending、清零尝试次数、立即到期。
    store.outbox_retry(&record_id, now).unwrap();
    let record = store.outbox_first().unwrap();
    assert_eq!(record.state, outbox_state::PENDING);
    assert_eq!(record.attempts, 0);
    assert_eq!(record.next_retry_at_ms, None);

    // 恢复成功路径：Done 落队列并回链 published。
    *outcome.lock().unwrap() = OutboxOutcome::Done {
        external_id: Some("memo-9".into()),
    };
    assert_eq!(host.run_outbox_tick(now), 1);
    assert_eq!(store.outbox_first().unwrap().state, outbox_state::DONE);
    let link = store.link("seg-1", "test").unwrap();
    assert_eq!(link.state, link_state::PUBLISHED);
    assert_eq!(link.external_id.as_deref(), Some("memo-9"));
}

#[test]
fn event_without_handler_is_deferred_six_hours() {
    let store = MockStore::new();
    let host = PluginHost::new(store.clone(), Arc::new(MockLogger::default()));
    store.outbox_enqueue(Some("k2"), "ghost.ping", "{}", NOW).unwrap();

    assert_eq!(host.run_outbox_tick(NOW), 1);
    let record = store.outbox_first().unwrap();
    assert_eq!(record.state, outbox_state::PENDING);
    assert_eq!(record.next_retry_at_ms, Some(NOW + 6 * 3_600_000));
    assert!(record.last_error.as_deref().unwrap().contains("处理器"));
}
