//! 状态机转换测试：覆盖 domain-model.md 的 T1–T9 与核心不变量。

use dotlog_domain::{
    finish, pause, punch, resume, start, undo, DomainError, Effect, Instant, NewIds, Phase,
    PointKind, PointSource, PunchMode, Segment, TimerState,
};

const T0: i64 = 1_700_000_000_000;
const TZ: i32 = 480;

fn at(minutes: i64) -> Instant {
    Instant::new(T0 + minutes * 60_000, TZ)
}

/// 在 `base_minute` 之后 `secs` 秒——用于构造 15 秒撤销窗口内的时间点。
fn after(base_minute: i64, secs: i64) -> Instant {
    Instant::new(T0 + base_minute * 60_000 + secs * 1_000, TZ)
}

fn src() -> PointSource {
    PointSource::Button
}

fn ids(segment: &str, point: &str) -> NewIds {
    NewIds::new(segment, point)
}

fn segment(id: &str, start: i64, end: Option<i64>) -> Segment {
    Segment {
        id: id.to_string(),
        start_at_ms: start,
        end_at_ms: end,
        title: String::new(),
        note: String::new(),
        tags: vec![],
        category_id: None,
        is_focus: false,
        is_ignored: false,
        tz_offset_min: TZ,
        revision: 1,
    }
}

// ---- T1 / T8 ----

#[test]
fn t1_start_creates_active_segment() {
    let t = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();

    assert_eq!(t.state.phase, Phase::Recording);
    let active = t.state.active.expect("应存在 active 段");
    assert_eq!(active.id, "seg-1");
    assert_eq!(active.start_at_ms, T0);
    assert_eq!(active.end_at_ms, None);
    assert_eq!(active.revision, 1);
    assert_eq!(active.tz_offset_min, TZ);
    assert_eq!(t.effects.len(), 2);
    assert!(t.undo.is_some());
    assert_eq!(t.pending_note_segment_id, None);
}

#[test]
fn t8_start_while_recording_is_rejected() {
    let first = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let err = start(&first.state, at(1), src(), ids("seg-2", "pt-101")).unwrap_err();
    assert_eq!(err, DomainError::ActiveExists);
    assert_eq!(err.code(), "E_ACTIVE_EXISTS");
}

// ---- T3 / T4 ----

#[test]
fn t3_punch_stop_closes_segment_and_returns_to_idle() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let punched = punch(
        &started.state,
        at(25),
        PunchMode::Stop,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap();

    assert_eq!(punched.state.phase, Phase::Idle);
    assert_eq!(punched.state.active, None);
    assert_eq!(punched.state.last_point_kind, Some(PointKind::Punch));
    assert_eq!(punched.pending_note_segment_id.as_deref(), Some("seg-1"));

    let closed = punched
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::UpdateSegment(s) => Some(s.clone()),
            _ => None,
        })
        .expect("应有被关闭段的更新");
    assert_eq!(closed.end_at_ms, Some(T0 + 25 * 60_000));
    assert_eq!(closed.revision, 2);
    closed.validate().unwrap();
}

#[test]
fn t4_punch_chain_opens_next_segment_at_same_instant() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let punched = punch(
        &started.state,
        at(30),
        PunchMode::Chain,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap();

    assert_eq!(punched.state.phase, Phase::Recording);
    let next = punched.state.active.expect("链式打点应开新段");
    assert_eq!(next.id, "seg-2");
    assert_eq!(next.start_at_ms, T0 + 30 * 60_000);
    assert_eq!(next.revision, 1);
    assert_eq!(punched.pending_note_segment_id.as_deref(), Some("seg-1"));
    assert_eq!(
        punched.undo.as_ref().unwrap().opened_segment_id.as_deref(),
        Some("seg-2")
    );
}

// ---- T5 / T6 / T2 ----

#[test]
fn t5_finish_closes_and_allows_note() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let done = finish(&started.state, at(90), src(), ids("", "pt-102")).unwrap();

    assert_eq!(done.state.phase, Phase::Idle);
    assert_eq!(done.state.last_point_kind, Some(PointKind::Finish));
    assert_eq!(done.pending_note_segment_id.as_deref(), Some("seg-1"));
}

#[test]
fn t6_pause_then_t2_resume() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let paused = pause(&started.state, at(10), src(), ids("", "pt-101")).unwrap();

    assert_eq!(paused.state.phase, Phase::Idle);
    assert!(paused.state.is_paused());
    assert_eq!(paused.pending_note_segment_id, None);

    let resumed = resume(&paused.state, at(20), src(), ids("seg-2", "pt-102")).unwrap();
    assert_eq!(resumed.state.phase, Phase::Recording);
    assert_eq!(resumed.state.active.unwrap().id, "seg-2");
    assert_eq!(resumed.state.last_point_kind, Some(PointKind::Resume));
}

#[test]
fn t7_punch_while_idle_is_rejected() {
    let err = punch(
        &TimerState::idle(),
        at(5),
        PunchMode::Stop,
        src(),
        ids("seg-1", "pt-100"),
    )
    .unwrap_err();
    assert_eq!(err, DomainError::NotRecording);
    assert_eq!(err.code(), "E_NOT_RECORDING");
}

#[test]
fn invalid_range_when_punch_at_start_time() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let err = punch(
        &started.state,
        at(0),
        PunchMode::Stop,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap_err();
    assert_eq!(err, DomainError::InvalidRange);
}

// ---- T9 撤销 ----

#[test]
fn t9_undo_start_removes_blank_active() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let token = started.undo.clone().unwrap();

    let undone = undo(&started.state, after(0, 1), &token, "pt-200".into(), src()).unwrap();
    assert_eq!(undone.state.phase, Phase::Idle);
    assert_eq!(undone.state.active, None);
    assert!(undone.effects.iter().any(|e| matches!(
        e,
        Effect::SoftDeleteSegment { id, .. } if id == "seg-1"
    )));
}

#[test]
fn t9_undo_punch_reopens_segment() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let punched = punch(
        &started.state,
        at(25),
        PunchMode::Stop,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap();
    let token = punched.undo.clone().unwrap();

    let undone = undo(&punched.state, after(25, 1), &token, "pt-201".into(), src()).unwrap();
    assert_eq!(undone.state.phase, Phase::Recording);
    let active = undone.state.active.unwrap();
    assert_eq!(active.id, "seg-1");
    assert_eq!(active.end_at_ms, None);
    assert_eq!(active.revision, 3); // 关闭 +1、重开 +1
}

#[test]
fn t9_undo_chain_removes_next_segment() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let punched = punch(
        &started.state,
        at(20),
        PunchMode::Chain,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap();
    let token = punched.undo.clone().unwrap();

    let undone = undo(&punched.state, after(20, 1), &token, "pt-202".into(), src()).unwrap();
    assert_eq!(undone.state.phase, Phase::Recording);
    assert_eq!(undone.state.active.unwrap().id, "seg-1");
    assert!(undone.effects.iter().any(|e| matches!(
        e,
        Effect::SoftDeleteSegment { id, .. } if id == "seg-2"
    )));
}

#[test]
fn t9_undo_chain_rejected_when_next_segment_has_content() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let punched = punch(
        &started.state,
        at(20),
        PunchMode::Chain,
        src(),
        ids("seg-2", "pt-101"),
    )
    .unwrap();
    let token = punched.undo.clone().unwrap();

    let mut state = punched.state.clone();
    state.active.as_mut().unwrap().title = "写代码".to_string();

    let err = undo(&state, after(20, 1), &token, "pt-202".into(), src()).unwrap_err();
    assert_eq!(err, DomainError::UndoUnavailable);
}

#[test]
fn t9_undo_expires_after_window() {
    let started = start(&TimerState::idle(), at(0), src(), ids("seg-1", "pt-100")).unwrap();
    let token = started.undo.clone().unwrap();

    // 窗口内（5 秒）应成功
    let ok = undo(
        &started.state,
        Instant::new(T0 + 5_000, TZ),
        &token,
        "pt-200".into(),
        src(),
    )
    .unwrap();
    assert_eq!(ok.state.phase, Phase::Idle);

    // 15 秒窗口：16 秒后必须拒绝
    let too_late = Instant::new(T0 + 16_000, TZ);
    let err = undo(&started.state, too_late, &token, "pt-201".into(), src()).unwrap_err();
    assert_eq!(err, DomainError::UndoExpired);
    assert_eq!(err.code(), "E_UNDO_EXPIRED");
}

// ---- 不变量 ----

#[test]
fn i2_closed_segment_must_end_after_start() {
    let seg = segment("seg-1", T0, Some(T0));
    assert_eq!(seg.validate().unwrap_err(), DomainError::InvalidRange);
}

#[test]
fn i3_overlap_uses_half_open_interval() {
    let a = segment("a", T0, Some(T0 + 60_000));
    let touching = segment("b", T0 + 60_000, Some(T0 + 120_000));
    let overlapping = segment("c", T0 + 59_999, Some(T0 + 150_000));

    assert!(!a.overlaps(&touching, T0 + 200_000));
    assert!(a.overlaps(&overlapping, T0 + 200_000));
}
