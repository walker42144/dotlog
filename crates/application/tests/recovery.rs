//! 启动核对、恢复处理与诊断快照的应用层测试。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dotlog_application::{
    AppResult, Clock, DiagnosticsView, PointRecord, RecoveryInfoView, TimerService, TimerSnapshot,
    TimerStore,
};
use dotlog_domain::recovery::{RecoveryAction, RecoveryReason};
use dotlog_domain::{
    Effect, Instant, Phase, PointKind, PointSource, PunchMode, Segment, TimerState,
};

const TZ: i32 = 480;
const MINUTE: i64 = 60_000;
const NOW: i64 = 1_700_000_000_000;

#[derive(Default)]
struct Data {
    active: Option<Segment>,
    segments: Vec<Segment>,
    points: Vec<PointRecord>,
    runtime: HashMap<String, String>,
}

#[derive(Clone, Default)]
struct FakeStore {
    inner: Arc<Mutex<Data>>,
}

impl FakeStore {
    fn with_active(start_ms: i64) -> Self {
        let store = Self::default();
        let segment = Segment::new_active("seg-active".to_string(), Instant::new(start_ms, TZ));
        {
            let mut data = store.inner.lock().unwrap();
            data.active = Some(segment.clone());
            data.segments.push(segment);
        }
        store
    }

    fn set_runtime(&self, key: &str, value: &str) {
        self.inner
            .lock()
            .unwrap()
            .runtime
            .insert(key.to_string(), value.to_string());
    }

    fn runtime(&self, key: &str) -> Option<String> {
        self.inner.lock().unwrap().runtime.get(key).cloned()
    }

    fn active(&self) -> Option<Segment> {
        self.inner.lock().unwrap().active.clone()
    }
}

impl TimerStore for FakeStore {
    fn snapshot(&self) -> AppResult<TimerSnapshot> {
        let data = self.inner.lock().unwrap();
        Ok(TimerSnapshot {
            active: data.active.clone(),
            last_point_kind: Some(PointKind::Start),
        })
    }

    fn commit(&self, effects: &[Effect], _now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for effect in effects {
            match effect {
                Effect::InsertSegment(segment) => {
                    data.segments.push(segment.clone());
                    if segment.is_active() {
                        data.active = Some(segment.clone());
                    }
                }
                Effect::UpdateSegment(segment) => {
                    data.segments.retain(|stored| stored.id != segment.id);
                    data.segments.push(segment.clone());
                    if data
                        .active
                        .as_ref()
                        .is_some_and(|active| active.id == segment.id && !segment.is_active())
                    {
                        data.active = None;
                    }
                }
                Effect::SoftDeleteSegment { id, .. } => {
                    data.segments.retain(|stored| &stored.id != id);
                    if data.active.as_ref().is_some_and(|active| &active.id == id) {
                        data.active = None;
                    }
                }
                Effect::RestoreSegment(_)
                | Effect::InsertTimePoint(_)
                | Effect::MarkPointUndone { .. } => {}
            }
        }
        Ok(())
    }

    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>> {
        let data = self.inner.lock().unwrap();
        Ok(data
            .segments
            .iter()
            .filter(|segment| {
                let end = segment.end_at_ms.unwrap_or(i64::MAX);
                segment.start_at_ms < to_ms && end > from_ms
            })
            .cloned()
            .collect())
    }

    fn find_segment(&self, id: &str) -> AppResult<Option<Segment>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .segments
            .iter()
            .find(|segment| segment.id == id)
            .cloned())
    }

    fn last_point(&self) -> AppResult<Option<PointRecord>> {
        Ok(self.inner.lock().unwrap().points.last().cloned())
    }

    fn history_segments(
        &self,
        _from_ms: i64,
        _to_ms: i64,
        _text: Option<&str>,
        _limit: u32,
        _offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)> {
        Ok((Vec::new(), 0))
    }

    fn settings(&self, _scope: &str) -> AppResult<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    fn runtime_state(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.runtime(key))
    }

    fn set_runtime_state(&self, key: &str, value: &str, _now_ms: i64) -> AppResult<()> {
        self.set_runtime(key, value);
        Ok(())
    }

    fn table_counts(&self) -> AppResult<Vec<(String, i64)>> {
        Ok(vec![
            ("time_segment".to_string(), 3),
            ("setting".to_string(), 1),
        ])
    }

    fn schema_version(&self) -> AppResult<i64> {
        Ok(2)
    }
}

#[derive(Clone)]
struct FixedClock(Arc<Mutex<Instant>>);

impl FixedClock {
    fn new(ms: i64) -> Self {
        Self(Arc::new(Mutex::new(Instant::new(ms, TZ))))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

fn service(store: FakeStore) -> TimerService<FakeStore, FixedClock> {
    TimerService::new(store, FixedClock::new(NOW), PunchMode::Stop)
}

#[test]
fn clean_recent_session_needs_no_recovery() {
    let store = FakeStore::with_active(NOW - 10 * MINUTE);
    store.set_runtime("heartbeat.wall_ms", &(NOW - MINUTE).to_string());
    store.set_runtime("lifecycle.clean_shutdown", "true");

    let report = service(store.clone()).begin_session().unwrap();

    assert!(!report.recovery.required);
    assert_eq!(report.recovery.reason, None);
    // 新会话立即标记为未正常退出。
    assert_eq!(
        store.runtime("lifecycle.clean_shutdown").as_deref(),
        Some("false")
    );
    assert_eq!(
        store.runtime("heartbeat.wall_ms").as_deref(),
        Some(NOW.to_string().as_str())
    );
}

#[test]
fn unclean_shutdown_requires_orphan_recovery() {
    let store = FakeStore::with_active(NOW - 10 * MINUTE);
    store.set_runtime("heartbeat.wall_ms", &(NOW - MINUTE).to_string());
    store.set_runtime("lifecycle.clean_shutdown", "false");

    let report = service(store).begin_session().unwrap();

    assert!(report.recovery.required);
    assert_eq!(report.recovery.reason, Some(RecoveryReason::OrphanActive));
    assert!(report.recovery.active_segment.is_some());
    assert_eq!(
        report.recovery.options,
        vec![
            RecoveryAction::Extend,
            RecoveryAction::Discard,
            RecoveryAction::Keep
        ]
    );
}

#[test]
fn stale_heartbeat_requires_long_gap_recovery() {
    // 起点落在当日窗口内，确保检测走到长间隔而非跨日分支。
    let store = FakeStore::with_active(NOW - 60 * MINUTE);
    store.set_runtime("heartbeat.wall_ms", &(NOW - 45 * MINUTE).to_string());
    store.set_runtime("lifecycle.clean_shutdown", "true");

    let report = service(store).begin_session().unwrap();

    assert!(report.recovery.required);
    assert_eq!(report.recovery.reason, Some(RecoveryReason::LongGap));
    assert!(report.recovery.options.contains(&RecoveryAction::Truncate));
}

#[test]
fn resolve_extend_closes_segment_and_marks_clean() {
    let store = FakeStore::with_active(NOW - 10 * MINUTE);
    store.set_runtime("heartbeat.wall_ms", &(NOW - MINUTE).to_string());
    store.set_runtime("lifecycle.clean_shutdown", "false");
    let timer = service(store.clone());
    let report = timer.begin_session().unwrap();

    let view = timer
        .resolve_recovery(
            report.recovery.reason.unwrap(),
            RecoveryAction::Extend,
            None,
        )
        .unwrap();

    assert_eq!(view.phase, Phase::Idle);
    assert!(view.active.is_none());
    assert!(store.active().is_none());
    assert_eq!(
        store.runtime("lifecycle.clean_shutdown").as_deref(),
        Some("false")
    );
}

#[test]
fn resolve_keep_preserves_recording_state() {
    let store = FakeStore::with_active(NOW - 10 * MINUTE);
    let timer = service(store.clone());
    let view = timer
        .resolve_recovery(RecoveryReason::OrphanActive, RecoveryAction::Keep, None)
        .unwrap();

    assert_eq!(view.phase, Phase::Recording);
    assert!(view.active.is_some());
    assert!(store.active().is_some());
}

#[test]
fn diagnostics_reports_counts_and_schema() {
    let timer = service(FakeStore::default());
    let view: DiagnosticsView = timer.diagnostics().unwrap();

    assert_eq!(view.schema_version, 2);
    assert_eq!(view.tables.len(), 2);
    assert!(view.integrity.quick_check_ok);
}

#[test]
fn end_session_marks_clean_shutdown() {
    let store = FakeStore::default();
    let timer = service(store.clone());
    timer.end_session().unwrap();

    assert_eq!(
        store.runtime("lifecycle.clean_shutdown").as_deref(),
        Some("true")
    );
}

#[test]
fn recovery_info_none_is_stable() {
    let info = RecoveryInfoView::none(NOW);
    assert!(!info.required);
    assert!(info.options.is_empty());
    assert!(info.active_segment.is_none());
}

// 保持 `TimerState` / `PointSource` 导入被使用，避免测试文件出现未使用告警。
#[allow(dead_code)]
fn _state_shape(state: TimerState) -> (Phase, Option<Segment>) {
    (state.phase, state.active)
}

#[allow(dead_code)]
fn _source() -> PointSource {
    PointSource::System
}
