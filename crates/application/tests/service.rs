//! 应用层用例测试：用内存假存储验证「命令 → 事务 → 视图」链路与错误码。

use std::sync::{Arc, Mutex};

use dotlog_application::{
    AppResult, CategoryRow, Clock, PointRecord, TimerService, TimerSnapshot, TimerStore,
};
use dotlog_domain::{Effect, Instant, Phase, PointKind, PointSource, PunchMode, Segment};

const T0: i64 = 1_700_000_000_000;
const TZ: i32 = 480;

#[derive(Default)]
struct FakeData {
    snapshot: TimerSnapshot,
    segments: Vec<Segment>,
    points: Vec<PointRecord>,
    categories: Vec<CategoryRow>,
    settings: Vec<(String, String, String)>,
}

#[derive(Clone, Default)]
struct FakeStore {
    inner: Arc<Mutex<FakeData>>,
}

impl TimerStore for FakeStore {
    fn snapshot(&self) -> AppResult<TimerSnapshot> {
        Ok(self.inner.lock().unwrap().snapshot.clone())
    }

    fn commit(&self, effects: &[Effect], _now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for effect in effects {
            match effect {
                Effect::InsertSegment(segment) => {
                    data.segments.push(segment.clone());
                    if segment.is_active() {
                        data.snapshot.active = Some(segment.clone());
                    }
                }
                Effect::UpdateSegment(segment) => {
                    data.segments.retain(|stored| stored.id != segment.id);
                    data.segments.push(segment.clone());
                    if segment.is_active() {
                        data.snapshot.active = Some(segment.clone());
                    } else if data
                        .snapshot
                        .active
                        .as_ref()
                        .is_some_and(|active| active.id == segment.id)
                    {
                        data.snapshot.active = None;
                    }
                }
                Effect::SoftDeleteSegment { id, .. } => {
                    data.segments.retain(|stored| &stored.id != id);
                    if data
                        .snapshot
                        .active
                        .as_ref()
                        .is_some_and(|active| &active.id == id)
                    {
                        data.snapshot.active = None;
                    }
                }
                Effect::RestoreSegment(segment) => {
                    data.segments.retain(|stored| stored.id != segment.id);
                    data.segments.push(segment.clone());
                    if segment.is_active() {
                        data.snapshot.active = Some(segment.clone());
                    } else if data
                        .snapshot
                        .active
                        .as_ref()
                        .is_some_and(|active| active.id == segment.id)
                    {
                        data.snapshot.active = None;
                    }
                }
                Effect::InsertTimePoint(point) => {
                    data.snapshot.last_point_kind = Some(point.kind);
                    data.points.push(PointRecord {
                        id: point.id.clone(),
                        kind: point.kind,
                        occurred_at_ms: point.occurred_at_ms,
                        segment_id: point.segment_id.clone(),
                        payload: point.payload.clone(),
                    });
                }
                Effect::MarkPointUndone { id, .. } => {
                    data.points.retain(|point| &point.id != id);
                    let last_kind = data.points.last().map(|point| point.kind);
                    data.snapshot.last_point_kind = last_kind;
                }
            }
        }
        Ok(())
    }

    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>> {
        let data = self.inner.lock().unwrap();
        let mut segments: Vec<Segment> = data
            .segments
            .iter()
            .filter(|segment| {
                let end = segment.end_at_ms.unwrap_or(i64::MAX);
                segment.start_at_ms < to_ms && end > from_ms
            })
            .cloned()
            .collect();
        segments.sort_by_key(|segment| segment.start_at_ms);
        Ok(segments)
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
        from_ms: i64,
        to_ms: i64,
        text: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)> {
        let data = self.inner.lock().unwrap();
        let mut matched: Vec<Segment> = data
            .segments
            .iter()
            .filter(|segment| {
                let end = segment.end_at_ms.unwrap_or(i64::MAX);
                segment.start_at_ms < to_ms && end > from_ms
            })
            .filter(|segment| {
                text.map_or(true, |needle| {
                    segment.title.contains(needle) || segment.note.contains(needle)
                })
            })
            .cloned()
            .collect();
        matched.sort_by_key(|segment| std::cmp::Reverse(segment.start_at_ms));
        let total = matched.len() as u64;
        let page = matched
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        Ok((page, total))
    }

    fn categories(&self) -> AppResult<Vec<CategoryRow>> {
        Ok(self.inner.lock().unwrap().categories.clone())
    }

    fn settings(&self, scope: &str) -> AppResult<Vec<(String, String)>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .settings
            .iter()
            .filter(|(stored_scope, _, _)| stored_scope == scope)
            .map(|(_, key, value)| (key.clone(), value.clone()))
            .collect())
    }

    fn settings_upsert(
        &self,
        scope: &str,
        entries: &[(String, String)],
        _now_ms: i64,
    ) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for (key, value) in entries {
            match data
                .settings
                .iter_mut()
                .find(|(stored_scope, stored_key, _)| stored_scope == scope && stored_key == key)
            {
                Some((_, _, stored_value)) => *stored_value = value.clone(),
                None => data
                    .settings
                    .push((scope.to_string(), key.clone(), value.clone())),
            }
        }
        Ok(())
    }

    fn settings_reset(&self, scope: &str, key: Option<&str>) -> AppResult<()> {
        let key = key.map(str::to_string);
        self.inner.lock().unwrap().settings.retain(|(stored_scope, stored_key, _)| {
            if stored_scope != scope {
                return true;
            }
            match &key {
                Some(key) => stored_key != key,
                None => false,
            }
        });
        Ok(())
    }
}

#[derive(Clone)]
struct FixedClock {
    now: Arc<Mutex<Instant>>,
}

impl FixedClock {
    fn new(ms: i64) -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::new(ms, TZ))),
        }
    }

    fn set(&self, ms: i64) {
        *self.now.lock().unwrap() = Instant::new(ms, TZ);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }
}

fn service(mode: PunchMode) -> (TimerService<FakeStore, FixedClock>, FakeStore, FixedClock) {
    let store = FakeStore::default();
    let clock = FixedClock::new(T0);
    (
        TimerService::new(store.clone(), clock.clone(), mode),
        store,
        clock,
    )
}

#[test]
fn get_state_is_idle_without_history() {
    let (service, _, _) = service(PunchMode::Stop);
    let state = service.get_state().unwrap();

    assert_eq!(state.phase, Phase::Idle);
    assert!(state.active.is_none());
    assert_eq!(state.elapsed_seconds, 0);
    assert!(!state.paused);
}

#[test]
fn start_switches_to_recording_and_counts_elapsed() {
    let (service, _, clock) = service(PunchMode::Stop);
    let started = service.start(PointSource::Button).unwrap();

    assert!(started.active.is_some());
    assert_eq!(started.elapsed_seconds, 0);

    clock.set(T0 + 90_000);
    let state = service.get_state().unwrap();
    assert_eq!(state.elapsed_seconds, 90);
    assert_eq!(state.active.unwrap().title, "");
}

#[test]
fn punch_stop_closes_segment_and_returns_to_idle() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 25 * 60_000);
    let result = service.punch(PointSource::Hotkey).unwrap();

    assert_eq!(result.closed_segment.duration_seconds, 25 * 60);
    assert!(result.new_segment.is_none());
    assert!(result.pending_note.is_some());
    assert!(!result.short_segment);
    assert_eq!(service.get_state().unwrap().phase, Phase::Idle);
}

#[test]
fn punch_chain_opens_next_segment() {
    let (service, _, clock) = service(PunchMode::Chain);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 10 * 60_000);
    let result = service.punch(PointSource::Button).unwrap();

    let next = result.new_segment.expect("链式打点应开新段");
    assert_eq!(next.start_at, T0 + 10 * 60_000);
    let state = service.get_state().unwrap();
    assert_eq!(state.phase, Phase::Recording);
    assert_eq!(state.active.unwrap().id, next.id);
}

#[test]
fn punch_shorter_than_threshold_is_marked() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 8_000);
    let result = service.punch(PointSource::Button).unwrap();
    assert!(result.short_segment);
}

#[test]
fn punch_while_idle_returns_stable_error_code() {
    let (service, _, _) = service(PunchMode::Stop);
    let error = service.punch(PointSource::Button).unwrap_err();
    assert_eq!(error.code(), "E_NOT_RECORDING");
}

#[test]
fn start_twice_reports_active_exists() {
    let (service, _, _) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    let error = service.start(PointSource::Button).unwrap_err();
    assert_eq!(error.code(), "E_ACTIVE_EXISTS");
}

#[test]
fn pause_then_start_resumes_recording() {
    let (service, store, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 5 * 60_000);
    service.pause(PointSource::Button).unwrap();
    assert!(service.get_state().unwrap().paused);

    clock.set(T0 + 30 * 60_000);
    let resumed = service.start(PointSource::Button).unwrap();
    assert_eq!(resumed.active.unwrap().start_at, T0 + 30 * 60_000);
    assert_eq!(
        store.snapshot().unwrap().last_point_kind,
        Some(PointKind::Resume)
    );
}

#[test]
fn finish_closes_segment() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 60 * 60_000);
    let result = service.finish(PointSource::Button).unwrap();
    assert_eq!(result.closed_segment.duration_seconds, 3_600);
    assert_eq!(service.get_state().unwrap().phase, Phase::Idle);
}

// ---- M1：撤销 / 日视图 / 补记 ----

#[test]
fn undo_punch_restores_recording() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    clock.set(T0 + 10 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap();
    assert!(punched.undo_token.is_some());

    let undone = service.undo().unwrap();
    let restored = undone.restored_segment.expect("撤销打点应重开段");
    assert_eq!(restored.id, punched.closed_segment.id);
    assert_eq!(restored.end_at, None);
    assert!(undone.removed_segment_id.is_none());

    let state = service.get_state().unwrap();
    assert_eq!(state.phase, Phase::Recording);
    assert_eq!(state.active.unwrap().id, punched.closed_segment.id);
}

#[test]
fn undo_start_removes_blank_segment() {
    let (service, _, _) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();

    let undone = service.undo().unwrap();
    assert!(undone.restored_segment.is_none());
    assert!(undone.removed_segment_id.is_some());
    assert_eq!(service.get_state().unwrap().phase, Phase::Idle);
}

#[test]
fn undo_after_window_returns_expired() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    service.punch(PointSource::Button).unwrap();

    clock.set(T0 + 10 * 60_000 + 16_000);
    let error = service.undo().unwrap_err();
    assert_eq!(error.code(), "E_UNDO_EXPIRED");
}

#[test]
fn undo_without_history_is_unavailable() {
    let (service, _, _) = service(PunchMode::Stop);
    let error = service.undo().unwrap_err();
    assert_eq!(error.code(), "E_UNDO_UNAVAILABLE");
}

#[test]
fn day_view_reports_segment_and_gap() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    service.punch(PointSource::Button).unwrap();

    let day = service.day(None).unwrap();
    assert_eq!(day.date, "2023-11-15");
    assert_eq!(day.segments.len(), 1);
    assert_eq!(day.segments[0].duration_seconds, 600);
    assert_eq!(day.stats.recorded_seconds, 600);
    assert_eq!(day.stats.effective_seconds, 0);
    assert_eq!(day.stats.segment_count, 1);
    assert_eq!(day.gaps.len(), 1);
    assert_eq!(day.gaps[0].end_at, T0);
}

#[test]
fn day_view_accepts_explicit_date() {
    let (service, _, _) = service(PunchMode::Stop);
    let day = service.day(Some("2026-10-28")).unwrap();
    assert_eq!(day.date, "2026-10-28");
    assert_eq!(day.window_end - day.window_start, 86_400_000);
    let error = service.day(Some("2026-02-31")).unwrap_err();
    assert_eq!(error.code(), "E_INVALID_RANGE");
}

#[test]
fn update_segment_sets_title_and_counts_effective() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap();

    let updated = service
        .update_segment(
            &punched.closed_segment.id,
            punched.closed_segment.revision,
            dotlog_application::dto::SegmentPatch {
                title: Some("写方案".into()),
                note: Some("思路整理".into()),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(updated.title, "写方案");
    assert_eq!(updated.revision, punched.closed_segment.revision + 1);
    let day = service.day(None).unwrap();
    assert_eq!(day.stats.effective_seconds, 600);
}

#[test]
fn update_segment_rejects_stale_revision() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap();

    let error = service
        .update_segment(
            &punched.closed_segment.id,
            punched.closed_segment.revision + 5,
            dotlog_application::dto::SegmentPatch {
                title: Some("迟到".into()),
                note: None,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "E_STALE_REVISION");
}

// ---- M2：编辑 / 补录 / 删除 / 拆分 / 合并 / 历史 ----

use dotlog_application::dto::BackfillInput;

fn patch() -> dotlog_application::dto::SegmentPatch {
    dotlog_application::dto::SegmentPatch::default()
}

#[test]
fn backfill_creates_segment_and_undo_removes_it() {
    let (service, _, clock) = service(PunchMode::Stop);
    let created = service
        .backfill_segment(BackfillInput {
            start_at: T0 - 60 * 60_000,
            end_at: T0 - 30 * 60_000,
            title: Some("晨会".into()),
            note: None,
            category_id: None,
            is_focus: false,
            is_ignored: false,
        })
        .unwrap();
    assert_eq!(created.duration_seconds, 1_800);
    assert_eq!(service.day(None).unwrap().stats.segment_count, 1);

    // 撤销补录：段消失。
    clock.set(T0 + 5_000);
    service.undo().unwrap();
    assert_eq!(service.day(None).unwrap().stats.segment_count, 0);
}

#[test]
fn backfill_overlap_returns_stable_code() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    service.punch(PointSource::Button).unwrap();

    clock.set(T0 + 20 * 60_000);
    let error = service
        .backfill_segment(BackfillInput {
            start_at: T0 + 5 * 60_000,
            end_at: T0 + 15 * 60_000,
            title: None,
            note: None,
            category_id: None,
            is_focus: false,
            is_ignored: false,
        })
        .unwrap_err();
    assert_eq!(error.code(), "E_OVERLAP");
}

#[test]
fn update_segment_resizes_and_detects_conflict() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let first = service.punch(PointSource::Button).unwrap().closed_segment;
    clock.set(T0 + 20 * 60_000);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 30 * 60_000);
    service.punch(PointSource::Button).unwrap();

    // 把第一段的终点拖进第二段 → E_OVERLAP。
    let error = service
        .update_segment(
            &first.id,
            first.revision,
            dotlog_application::dto::SegmentPatch {
                end_at: Some(T0 + 25 * 60_000),
                ..patch()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "E_OVERLAP");

    // 合法拖动：终点移到 15 分处。
    let updated = service
        .update_segment(
            &first.id,
            first.revision,
            dotlog_application::dto::SegmentPatch {
                end_at: Some(T0 + 15 * 60_000),
                ..patch()
            },
        )
        .unwrap();
    assert_eq!(updated.end_at, Some(T0 + 15 * 60_000));
    assert_eq!(updated.duration_seconds, 900);

    // 时间编辑也可撤销。
    clock.set(T0 + 30 * 60_000 + 5_000);
    service.undo().unwrap();
    let day = service.day(None).unwrap();
    let restored = day.segments.iter().find(|s| s.id == first.id).unwrap();
    assert_eq!(restored.duration_seconds, 600);
}

#[test]
fn split_then_undo_restores_original() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 20 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap().closed_segment;

    let parts = service
        .split_segment(&punched.id, T0 + 10 * 60_000)
        .unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].duration_seconds, 600);
    assert_eq!(parts[1].duration_seconds, 600);
    assert_eq!(service.day(None).unwrap().stats.segment_count, 2);
    // 总时长守恒。
    assert_eq!(service.day(None).unwrap().stats.recorded_seconds, 1_200);

    clock.set(T0 + 20 * 60_000 + 5_000);
    service.undo().unwrap();
    let day = service.day(None).unwrap();
    assert_eq!(day.stats.segment_count, 1);
    assert_eq!(day.stats.recorded_seconds, 1_200);
}

#[test]
fn merge_then_undo_restores_originals() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let a = service.punch(PointSource::Button).unwrap().closed_segment;
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 25 * 60_000);
    let b = service.punch(PointSource::Button).unwrap().closed_segment;

    let merged = service.merge_segments(&[a.id.clone(), b.id.clone()]).unwrap();
    assert_eq!(merged.start_at, a.start_at);
    assert_eq!(merged.end_at, b.end_at);
    assert_eq!(merged.duration_seconds, 1_500);
    assert_eq!(service.day(None).unwrap().stats.segment_count, 1);

    clock.set(T0 + 25 * 60_000 + 5_000);
    service.undo().unwrap();
    let day = service.day(None).unwrap();
    assert_eq!(day.stats.segment_count, 2);
    assert_eq!(day.stats.recorded_seconds, 1_500);
}

#[test]
fn delete_then_undo_restores_segment() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap().closed_segment;

    clock.set(T0 + 20 * 60_000);
    service.delete_segment(&punched.id).unwrap();
    assert_eq!(service.day(None).unwrap().stats.segment_count, 0);

    clock.set(T0 + 20 * 60_000 + 5_000);
    service.undo().unwrap();
    let day = service.day(None).unwrap();
    assert_eq!(day.stats.segment_count, 1);
    assert_eq!(day.stats.recorded_seconds, 600);
}

#[test]
fn delete_active_returns_to_idle_and_undo_resumes() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let active_id = service.get_state().unwrap().active.unwrap().id;

    service.delete_segment(&active_id).unwrap();
    assert_eq!(service.get_state().unwrap().phase, Phase::Idle);

    clock.set(T0 + 10 * 60_000 + 5_000);
    service.undo().unwrap();
    let state = service.get_state().unwrap();
    assert_eq!(state.phase, Phase::Recording);
    assert_eq!(state.active.unwrap().id, active_id);
}

#[test]
fn history_returns_desc_list_with_total_and_text_filter() {
    let (service, _, clock) = service(PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 10 * 60_000);
    let a = service.punch(PointSource::Button).unwrap().closed_segment;
    service
        .update_segment(
            &a.id,
            a.revision,
            dotlog_application::dto::SegmentPatch {
                title: Some("英语".into()),
                ..patch()
            },
        )
        .unwrap();
    clock.set(T0 + 20 * 60_000);
    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 30 * 60_000);
    service.punch(PointSource::Button).unwrap();

    let all = service.history(T0 - 1, T0 + 3_600_000, None, 50, 0).unwrap();
    assert_eq!(all.total, 2);
    assert_eq!(all.items.len(), 2);
    assert!(all.items[0].start_at > all.items[1].start_at);

    let filtered = service
        .history(T0 - 1, T0 + 3_600_000, Some("英语"), 50, 0)
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items[0].title, "英语");
}
