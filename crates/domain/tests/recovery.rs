//! 恢复检测与处理规则的单元测试（domain-model §8 / §12）。

use dotlog_domain::recovery::{
    detect, resolve, RecoveryAction, RecoveryContext, RecoveryReason, RecoveryResolution,
};
use dotlog_domain::{Effect, Instant, Phase, PointKind, PointSource, Segment, TimerState};

const TZ: i32 = 480;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 3_600_000;
const NOW: i64 = 1_700_000_000_000;
const DAY_START: i64 = NOW - 3 * HOUR;

fn active(start_ms: i64) -> Segment {
    Segment::new_active("seg-active".to_string(), Instant::new(start_ms, TZ))
}

fn state(segment: &Segment) -> TimerState {
    TimerState {
        phase: Phase::Recording,
        active: Some(segment.clone()),
        last_point_kind: Some(PointKind::Start),
    }
}

fn context(now_ms: i64, heartbeat: Option<i64>, clean: bool) -> RecoveryContext {
    RecoveryContext {
        now_ms,
        day_window_start_ms: DAY_START,
        last_heartbeat_ms: heartbeat,
        clean_shutdown: clean,
        ..RecoveryContext::default()
    }
}

#[test]
fn cross_day_active_requires_recovery() {
    let segment = active(DAY_START - HOUR);
    let check = detect(Some(&segment), &context(NOW, Some(NOW - MINUTE), true));

    assert!(check.required);
    assert_eq!(check.reason, Some(RecoveryReason::CrossDay));
    assert_eq!(check.cutoff_ms, Some(DAY_START));
    assert_eq!(
        check.options(),
        vec![
            RecoveryAction::Truncate,
            RecoveryAction::Extend,
            RecoveryAction::Discard
        ]
    );
}

#[test]
fn cross_day_wins_over_long_gap() {
    let segment = active(DAY_START - HOUR);
    // 心跳也很久没更新：跨日优先级更高。
    let check = detect(Some(&segment), &context(NOW, Some(NOW - 2 * HOUR), true));

    assert_eq!(check.reason, Some(RecoveryReason::CrossDay));
}

#[test]
fn long_gap_uses_last_heartbeat_as_cutoff() {
    let segment = active(NOW - 3 * HOUR);
    let heartbeat = NOW - HOUR;
    let check = detect(Some(&segment), &context(NOW, Some(heartbeat), true));

    assert!(check.required);
    assert_eq!(check.reason, Some(RecoveryReason::LongGap));
    assert_eq!(check.cutoff_ms, Some(heartbeat));
}

#[test]
fn long_gap_without_heartbeat_has_no_truncate_option() {
    let segment = active(NOW - HOUR);
    let check = detect(Some(&segment), &context(NOW, None, true));

    assert_eq!(check.reason, Some(RecoveryReason::LongGap));
    assert_eq!(check.cutoff_ms, None);
    assert_eq!(
        check.options(),
        vec![RecoveryAction::Extend, RecoveryAction::Discard]
    );
}

#[test]
fn future_start_is_clock_anomaly() {
    let segment = active(NOW + 5 * MINUTE);
    let check = detect(Some(&segment), &context(NOW, Some(NOW - MINUTE), true));

    assert!(check.required);
    assert_eq!(check.reason, Some(RecoveryReason::ClockAnomaly));
    assert_eq!(
        check.options(),
        vec![RecoveryAction::Extend, RecoveryAction::Keep]
    );
}

#[test]
fn backward_clock_without_active_only_hints() {
    let check = detect(None, &context(NOW, Some(NOW + 10 * MINUTE), true));

    assert!(!check.required);
    assert_eq!(check.reason, Some(RecoveryReason::ClockAnomaly));
}

#[test]
fn recent_clean_session_needs_nothing() {
    let segment = active(NOW - 10 * MINUTE);
    let check = detect(Some(&segment), &context(NOW, Some(NOW - MINUTE), true));

    assert!(!check.required);
    assert_eq!(check.reason, None);
}

#[test]
fn unclean_shutdown_leaves_orphan_active() {
    let segment = active(NOW - 10 * MINUTE);
    let check = detect(Some(&segment), &context(NOW, Some(NOW - MINUTE), false));

    assert!(check.required);
    assert_eq!(check.reason, Some(RecoveryReason::OrphanActive));
    assert_eq!(
        check.options(),
        vec![
            RecoveryAction::Extend,
            RecoveryAction::Discard,
            RecoveryAction::Keep
        ]
    );
}

#[test]
fn extend_closes_at_now_and_writes_system_fact() {
    let segment = active(NOW - 30 * MINUTE);
    let transition = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::LongGap,
            action: RecoveryAction::Extend,
            at: Instant::new(NOW, TZ),
            cutoff_ms: None,
            point_id: "pt-recovery".to_string(),
        },
    )
    .unwrap();

    assert_eq!(transition.state.phase, Phase::Idle);
    assert!(transition.state.active.is_none());
    assert_eq!(transition.effects.len(), 2);

    let closed = match &transition.effects[0] {
        Effect::UpdateSegment(segment) => segment,
        other => panic!("首个效果应为更新段，实际 {other:?}"),
    };
    assert_eq!(closed.end_at_ms, Some(NOW));

    let point = match &transition.effects[1] {
        Effect::InsertTimePoint(point) => point,
        other => panic!("第二个效果应为时间点，实际 {other:?}"),
    };
    assert_eq!(point.kind, PointKind::Finish);
    assert_eq!(point.source, PointSource::System);
    assert!(point
        .payload
        .as_deref()
        .is_some_and(|payload| payload.contains("long_gap")));
}

#[test]
fn truncate_closes_at_cutoff() {
    let segment = active(NOW - HOUR);
    let cutoff = NOW - 20 * MINUTE;
    let transition = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::LongGap,
            action: RecoveryAction::Truncate,
            at: Instant::new(NOW, TZ),
            cutoff_ms: Some(cutoff),
            point_id: "pt-recovery".to_string(),
        },
    )
    .unwrap();

    let closed = match &transition.effects[0] {
        Effect::UpdateSegment(segment) => segment,
        other => panic!("首个效果应为更新段，实际 {other:?}"),
    };
    assert_eq!(closed.end_at_ms, Some(cutoff));
}

#[test]
fn discard_soft_deletes_active_segment() {
    let segment = active(NOW - 20 * MINUTE);
    let transition = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::OrphanActive,
            action: RecoveryAction::Discard,
            at: Instant::new(NOW, TZ),
            cutoff_ms: None,
            point_id: "pt-recovery".to_string(),
        },
    )
    .unwrap();

    assert_eq!(transition.state.phase, Phase::Idle);
    assert!(matches!(
        transition.effects[0],
        Effect::SoftDeleteSegment { .. }
    ));
    match &transition.effects[1] {
        Effect::InsertTimePoint(point) => assert_eq!(point.kind, PointKind::Delete),
        other => panic!("第二个效果应为时间点，实际 {other:?}"),
    }
}

#[test]
fn keep_leaves_state_untouched() {
    let segment = active(NOW - 10 * MINUTE);
    let transition = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::OrphanActive,
            action: RecoveryAction::Keep,
            at: Instant::new(NOW, TZ),
            cutoff_ms: None,
            point_id: "pt-recovery".to_string(),
        },
    )
    .unwrap();

    assert_eq!(transition.state.phase, Phase::Recording);
    assert!(transition.effects.is_empty());
}

#[test]
fn action_not_allowed_for_reason_is_rejected() {
    let segment = active(NOW - 10 * MINUTE);
    let result = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::ClockAnomaly,
            action: RecoveryAction::Discard,
            at: Instant::new(NOW, TZ),
            cutoff_ms: None,
            point_id: "pt-recovery".to_string(),
        },
    );

    assert!(result.is_err());
}

#[test]
fn truncate_before_start_is_rejected() {
    let segment = active(NOW - 10 * MINUTE);
    let result = resolve(
        &state(&segment),
        &RecoveryResolution {
            reason: RecoveryReason::CrossDay,
            action: RecoveryAction::Truncate,
            at: Instant::new(NOW, TZ),
            cutoff_ms: Some(NOW - 30 * MINUTE),
            point_id: "pt-recovery".to_string(),
        },
    );

    assert!(result.is_err());
}
