//! 编辑与补录的领域规则测试（docs/domain-model.md §9、§12）。

use dotlog_domain::{
    backfill, delete, merge, resize, split, undo, BackfillParams, DomainError, EditUndo, Instant,
    NewIds, PointKind, PointSource, Segment, TimerState, UndoToken, DEFAULT_UNDO_WINDOW_MS,
};

const T0: i64 = 1_700_000_000_000;
const TZ: i32 = 480;
const HOUR: i64 = 3_600_000;

fn at(ms: i64) -> Instant {
    Instant::new(ms, TZ)
}

fn ids(n: u64) -> NewIds {
    NewIds::new(format!("seg-{n}"), format!("pt-{n}"))
}

fn closed_segment(id: &str, start: i64, end: i64) -> Segment {
    Segment {
        id: id.to_string(),
        start_at_ms: start,
        end_at_ms: Some(end),
        title: String::new(),
        note: String::new(),
        tags: Vec::new(),
        category_id: None,
        is_focus: false,
        is_ignored: false,
        tz_offset_min: TZ,
        revision: 1,
    }
}

fn idle() -> TimerState {
    TimerState::idle()
}

// ---- 补录 ----

#[test]
fn backfill_creates_closed_segment_and_undo_removes_it() {
    let transition = backfill(
        &idle(),
        &[],
        BackfillParams {
            start_ms: T0,
            end_ms: T0 + HOUR,
            title: "补录：开会".into(),
            note: String::new(),
            at: at(T0 + 2 * HOUR),
            ids: ids(1),
            source: PointSource::Button,
        },
    )
    .expect("补录应成功");

    let inserted = transition
        .effects
        .iter()
        .find_map(|effect| match effect {
            dotlog_domain::Effect::InsertSegment(segment) => Some(segment),
            _ => None,
        })
        .expect("应插入新段");
    assert_eq!(inserted.end_at_ms, Some(T0 + HOUR));
    assert_eq!(inserted.title, "补录：开会");

    // 撤销补录 = 软删除新建段。
    let token = transition.undo.expect("补录应可撤销");
    let undone = undo(
        &transition.state,
        at(T0 + 2 * HOUR + 5_000),
        &token,
        "pt-undo".into(),
        PointSource::Button,
    )
    .expect("撤销应成功");
    assert!(undone
        .effects
        .iter()
        .any(|effect| matches!(
            effect,
            dotlog_domain::Effect::SoftDeleteSegment { id, .. } if id == &inserted.id
        )));
}

#[test]
fn backfill_rejects_overlap_and_returns_conflicts() {
    let existing = closed_segment("seg-x", T0 + 500, T0 + 1_500);
    let result = backfill(
        &idle(),
        std::slice::from_ref(&existing),
        BackfillParams {
            start_ms: T0,
            end_ms: T0 + 2_000,
            title: String::new(),
            note: String::new(),
            at: at(T0 + 3_000),
            ids: ids(2),
            source: PointSource::Button,
        },
    );
    assert_eq!(result, Err(DomainError::Overlap(vec![existing])));
}

#[test]
fn backfill_allows_touching_neighbors() {
    // 半开区间：与前段首尾相接不算重叠（§12 用例）。
    let result = backfill(
        &idle(),
        &[],
        BackfillParams {
            start_ms: T0,
            end_ms: T0 + HOUR,
            title: String::new(),
            note: String::new(),
            at: at(T0 + 2 * HOUR),
            ids: ids(3),
            source: PointSource::Button,
        },
    );
    assert!(result.is_ok());
}

#[test]
fn backfill_rejects_future_and_too_long() {
    let future = backfill(
        &idle(),
        &[],
        BackfillParams {
            start_ms: T0,
            end_ms: T0 + 3 * HOUR,
            title: String::new(),
            note: String::new(),
            at: at(T0 + 2 * HOUR),
            ids: ids(4),
            source: PointSource::Button,
        },
    );
    assert_eq!(future, Err(DomainError::FutureTime));

    let too_long = backfill(
        &idle(),
        &[],
        BackfillParams {
            start_ms: T0,
            end_ms: T0 + 25 * HOUR,
            title: String::new(),
            note: String::new(),
            at: at(T0 + 26 * HOUR),
            ids: ids(5),
            source: PointSource::Button,
        },
    );
    assert_eq!(too_long, Err(DomainError::BackfillTooLong));
}

// ---- 修改起止 ----

#[test]
fn resize_moves_edges_within_neighbors() {
    let segment = closed_segment("seg-a", T0 + HOUR, T0 + 2 * HOUR);
    let updated = resize(&segment, &[], T0 + HOUR / 2, Some(T0 + 3 * HOUR), at(T0 + 4 * HOUR))
        .expect("应向两侧扩展成功");
    assert_eq!(updated.start_at_ms, T0 + HOUR / 2);
    assert_eq!(updated.end_at_ms, Some(T0 + 3 * HOUR));
    assert_eq!(updated.revision, 2);
}

#[test]
fn resize_rejects_conflict_end_before_start_and_future() {
    let segment = closed_segment("seg-a", T0, T0 + HOUR);
    let neighbor = closed_segment("seg-b", T0 + 2 * HOUR, T0 + 3 * HOUR);

    let conflict = resize(
        &segment,
        std::slice::from_ref(&neighbor),
        T0,
        Some(T0 + 2 * HOUR + 1),
        at(T0 + 4 * HOUR),
    );
    assert!(matches!(conflict, Err(DomainError::Overlap(_))));

    let reversed = resize(&segment, &[], T0 + HOUR, Some(T0), at(T0 + 4 * HOUR));
    assert_eq!(reversed, Err(DomainError::InvalidRange));

    let future = resize(&segment, &[], T0 + 5 * HOUR, Some(T0 + 6 * HOUR), at(T0 + 4 * HOUR));
    assert_eq!(future, Err(DomainError::FutureTime));
}

#[test]
fn resize_active_segment_cannot_set_end() {
    let active = Segment::new_active("seg-active".into(), at(T0));
    let result = resize(&active, &[], T0, Some(T0 + HOUR), at(T0 + 2 * HOUR));
    assert_eq!(result, Err(DomainError::ActiveHasNoEnd));

    // 但可以前移起点。
    let moved = resize(&active, &[], T0 - HOUR, None, at(T0 + HOUR)).expect("active 可改起点");
    assert_eq!(moved.start_at_ms, T0 - HOUR);
    assert!(moved.is_active());
}

// ---- 拆分 ----

#[test]
fn split_divides_segment_and_preserves_content() {
    let mut segment = closed_segment("seg-a", T0, T0 + 2 * HOUR);
    segment.title = "写文档".into();

    let (first, second) =
        split(&segment, T0 + HOUR, "seg-new".into(), T0 + 3 * HOUR).expect("拆分应成功");
    assert_eq!(first.end_at_ms, Some(T0 + HOUR));
    assert_eq!(first.revision, 2);
    assert_eq!(second.id, "seg-new");
    assert_eq!(second.start_at_ms, T0 + HOUR);
    assert_eq!(second.end_at_ms, Some(T0 + 2 * HOUR));
    assert_eq!(second.title, "写文档");
    // 两段时长之和等于原段。
    assert_eq!(
        first.duration_ms(0) + second.duration_ms(0),
        segment.duration_ms(0)
    );
}

#[test]
fn split_rejects_point_outside_segment() {
    let segment = closed_segment("seg-a", T0, T0 + HOUR);
    assert_eq!(
        split(&segment, T0, "x".into(), T0 + HOUR),
        Err(DomainError::InvalidRange)
    );
    assert_eq!(
        split(&segment, T0 + HOUR, "x".into(), T0 + HOUR),
        Err(DomainError::InvalidRange)
    );
}

#[test]
fn split_active_segment_keeps_second_half_active() {
    let active = Segment::new_active("seg-active".into(), at(T0));
    let (first, second) =
        split(&active, T0 + HOUR, "seg-new".into(), T0 + 2 * HOUR).expect("拆分 active 应成功");
    assert!(!first.is_active());
    assert!(second.is_active());
}

// ---- 合并 ----

#[test]
fn merge_covers_gap_and_joins_content() {
    let mut a = closed_segment("seg-a", T0, T0 + HOUR);
    a.title = "英语".into();
    let mut b = closed_segment("seg-b", T0 + 2 * HOUR, T0 + 3 * HOUR);
    b.title = "阅读".into();

    let merged = merge(vec![a, b], "seg-m".into()).expect("合并应成功");
    assert_eq!(merged.start_at_ms, T0);
    assert_eq!(merged.end_at_ms, Some(T0 + 3 * HOUR));
    assert_eq!(merged.title, "英语、阅读");
}

#[test]
fn merge_rejects_single_segment_and_active() {
    let a = closed_segment("seg-a", T0, T0 + HOUR);
    assert_eq!(merge(vec![a.clone()], "m".into()), Err(DomainError::MergeNeedsTwo));

    let active = Segment::new_active("seg-active".into(), at(T0 + 2 * HOUR));
    assert_eq!(merge(vec![a, active], "m".into()), Err(DomainError::MergeActive));
}

// ---- 删除 ----

#[test]
fn delete_closed_segment_is_undoable() {
    let segment = closed_segment("seg-a", T0, T0 + HOUR);
    let transition = delete(
        &idle(),
        &segment,
        at(T0 + 2 * HOUR),
        PointSource::Button,
        "pt-del".into(),
        "pt-fin".into(),
    )
    .expect("删除应成功");

    // 已关闭段删除不产生 finish 事实。
    assert_eq!(
        transition
            .effects
            .iter()
            .filter(|effect| matches!(effect, dotlog_domain::Effect::InsertTimePoint(_)))
            .count(),
        1
    );

    let token = transition.undo.expect("删除应可撤销");
    let undone = undo(
        &transition.state,
        at(T0 + 2 * HOUR + 5_000),
        &token,
        "pt-undo".into(),
        PointSource::Button,
    )
    .expect("撤销删除应成功");
    assert!(undone
        .effects
        .iter()
        .any(|effect| matches!(
            effect,
            dotlog_domain::Effect::RestoreSegment(restored) if restored.id == "seg-a"
        )));
}

#[test]
fn delete_active_closes_then_deletes_and_undo_restores_active() {
    let active = Segment::new_active("seg-active".into(), at(T0));
    let state = TimerState::recording(active.clone());

    let transition = delete(
        &state,
        &active,
        at(T0 + HOUR),
        PointSource::Button,
        "pt-del".into(),
        "pt-fin".into(),
    )
    .expect("删除 active 应成功");
    assert!(!transition.state.is_recording());
    // active 删除 = finish + delete 两条事实。
    assert_eq!(
        transition
            .effects
            .iter()
            .filter(|effect| matches!(effect, dotlog_domain::Effect::InsertTimePoint(_)))
            .count(),
        2
    );

    let token = transition.undo.expect("删除应可撤销");
    let undone = undo(
        &transition.state,
        at(T0 + HOUR + 5_000),
        &token,
        "pt-undo".into(),
        PointSource::Button,
    )
    .expect("撤销应成功");
    assert!(undone.state.is_recording());
    assert_eq!(undone.state.active.as_ref().map(|s| s.id.as_str()), Some("seg-active"));
}

#[test]
fn undo_edit_restores_snapshot_and_removes_created() {
    let original = closed_segment("seg-a", T0, T0 + HOUR);
    let state = idle();
    let token = UndoToken {
        point_id: "pt-edit".into(),
        kind: PointKind::Edit,
        issued_at_ms: T0 + 2 * HOUR,
        window_ms: DEFAULT_UNDO_WINDOW_MS,
        closed_segment: None,
        opened_segment_id: None,
        edit: Some(EditUndo {
            restore: vec![original.clone()],
            remove_ids: vec!["seg-new".into()],
        }),
    };

    let undone = undo(
        &state,
        at(T0 + 2 * HOUR + 5_000),
        &token,
        "pt-undo".into(),
        PointSource::Button,
    )
    .expect("编辑撤销应成功");

    let restored = undone.effects.iter().find_map(|effect| match effect {
        dotlog_domain::Effect::RestoreSegment(segment) => Some(segment),
        _ => None,
    });
    assert_eq!(restored.map(|segment| segment.revision), Some(original.revision + 1));
    assert!(undone
        .effects
        .iter()
        .any(|effect| matches!(
            effect,
            dotlog_domain::Effect::SoftDeleteSegment { id, .. } if id == "seg-new"
        )));
    // 与计时无关的编辑撤销不改变相位。
    assert!(!undone.state.is_recording());
}
