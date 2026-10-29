//! 端到端用例：`TimerService` + 真实 SQLite，覆盖 M1 的打点、撤销、补记与日视图落库。

use std::sync::{Arc, Mutex};

use dotlog_application::dto::SegmentPatch;
use dotlog_application::{Clock, TimerService};
use dotlog_domain::{Instant, PointSource, PunchMode};
use dotlog_storage::Database;

const T0: i64 = 1_700_000_000_000;
const TZ: i32 = 480;

#[derive(Clone)]
struct FixedClock {
    now: Arc<Mutex<i64>>,
}

impl FixedClock {
    fn new() -> Self {
        Self {
            now: Arc::new(Mutex::new(T0)),
        }
    }

    fn set(&self, ms: i64) {
        *self.now.lock().unwrap() = ms;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        Instant::new(*self.now.lock().unwrap(), TZ)
    }
}

#[test]
fn punch_undo_edit_and_day_view_round_trip() {
    let db = Database::open_in_memory(T0).unwrap();
    let clock = FixedClock::new();
    let service = TimerService::new(db, clock.clone(), PunchMode::Stop);

    service.start(PointSource::Button).unwrap();
    clock.set(T0 + 25 * 60_000);
    let punched = service.punch(PointSource::Button).unwrap();
    assert!(punched.undo_token.is_some());

    let day = service.day(None).unwrap();
    assert_eq!(day.stats.recorded_seconds, 25 * 60);
    assert_eq!(day.segments.len(), 1);
    assert_eq!(day.segments[0].title, "");

    // 15 秒窗口内撤销：段重开，事实被标记为已撤销。
    clock.set(T0 + 25 * 60_000 + 1_000);
    let undone = service.undo().unwrap();
    let restored = undone.restored_segment.expect("撤销打点应重开段");
    assert_eq!(restored.end_at, None);

    // 再次打点并补记标题，日视图应识别为有效记录。
    clock.set(T0 + 30 * 60_000);
    let closed = service.punch(PointSource::Button).unwrap().closed_segment;
    clock.set(T0 + 30 * 60_000 + 2_000);
    let patched = service
        .update_segment(
            &closed.id,
            closed.revision,
            SegmentPatch {
                title: Some("整理周报".to_string()),
                note: Some("M1 验收".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(patched.title, "整理周报");

    let day = service.day(None).unwrap();
    assert_eq!(day.segments.len(), 1);
    assert_eq!(day.segments[0].title, "整理周报");
    assert_eq!(day.stats.effective_seconds, day.stats.recorded_seconds);
    assert!(day.gaps.iter().any(|gap| gap.end_at == T0));
}

/// M2 全链路：补录 → 改时间 → 拆分 → 合并 → 删除 → 逐步撤销，真实 SQLite 落库。
#[test]
fn edit_flow_with_real_sqlite_round_trip() {
    use dotlog_application::dto::BackfillInput;

    let db = Database::open_in_memory(T0).unwrap();
    let clock = FixedClock::new();
    let service = TimerService::new(db, clock.clone(), PunchMode::Stop);

    // 补录两小时前的一段。
    let backfilled = service
        .backfill_segment(BackfillInput {
            start_at: T0 - 2 * 3_600_000,
            end_at: T0 - 3_600_000,
            title: Some("补录：晨跑".to_string()),
            note: None,
            category_id: None,
            is_focus: false,
            is_ignored: false,
        })
        .unwrap();
    assert_eq!(service.day(None).unwrap().stats.recorded_seconds, 3_600);

    // 改终点：缩短到 45 分钟。
    clock.set(T0 + 1_000);
    let resized = service
        .update_segment(
            &backfilled.id,
            backfilled.revision,
            SegmentPatch {
                end_at: Some(T0 - 2 * 3_600_000 + 45 * 60_000),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(resized.duration_seconds, 2_700);

    // 与自身相交不算冲突：终点不动只改标题。
    clock.set(T0 + 2_000);
    let renamed = service
        .update_segment(
            &backfilled.id,
            resized.revision,
            SegmentPatch {
                title: Some("晨跑".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(renamed.title, "晨跑");

    // 拆分：45 分钟 → 20 + 25。
    clock.set(T0 + 3_000);
    let parts = service
        .split_segment(&backfilled.id, T0 - 2 * 3_600_000 + 20 * 60_000)
        .unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(service.day(None).unwrap().stats.recorded_seconds, 2_700);

    // 合并回去。
    clock.set(T0 + 4_000);
    let merged = service
        .merge_segments(&[parts[0].id.clone(), parts[1].id.clone()])
        .unwrap();
    assert_eq!(merged.duration_seconds, 2_700);
    assert_eq!(service.day(None).unwrap().stats.segment_count, 1);

    // 删除后撤销，段恢复。
    clock.set(T0 + 5_000);
    service.delete_segment(&merged.id).unwrap();
    assert_eq!(service.day(None).unwrap().stats.segment_count, 0);
    clock.set(T0 + 6_000);
    service.undo().unwrap();
    let day = service.day(None).unwrap();
    assert_eq!(day.stats.segment_count, 1);
    assert_eq!(day.stats.recorded_seconds, 2_700);

    // 历史查询能按文本找到。
    let history = service
        .history(T0 - 3 * 3_600_000, T0, Some("晨跑"), 50, 0)
        .unwrap();
    assert_eq!(history.total, 1);
    assert_eq!(history.items[0].title, "晨跑");
}
