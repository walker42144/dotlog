use crate::error::{DomainError, DomainResult};
use crate::model::{
    Effect, Instant, NewIds, Phase, PointKind, PointSource, PunchMode, Segment, SegmentId,
    TimePoint, TimePointId, TimerState, UndoToken, DEFAULT_UNDO_WINDOW_MS,
};

/// T1：开始记录，新建 active 段。
pub fn start(
    state: &TimerState,
    at: Instant,
    source: PointSource,
    ids: NewIds,
) -> DomainResult<Transition> {
    if state.phase == Phase::Recording || state.active.is_some() {
        return Err(DomainError::ActiveExists);
    }
    open_segment(at, source, ids, PointKind::Start)
}

/// T2：暂停后继续，行为同 start，仅事实类型不同。
pub fn resume(
    state: &TimerState,
    at: Instant,
    source: PointSource,
    ids: NewIds,
) -> DomainResult<Transition> {
    if state.phase == Phase::Recording || state.active.is_some() {
        return Err(DomainError::ActiveExists);
    }
    if !state.is_paused() {
        return Err(DomainError::NotRecording);
    }
    open_segment(at, source, ids, PointKind::Resume)
}

/// T3 / T4：打点。`Stop` 关闭当前段回 Idle，`Chain` 关闭后立即开新段。
pub fn punch(
    state: &TimerState,
    at: Instant,
    mode: PunchMode,
    source: PointSource,
    ids: NewIds,
) -> DomainResult<Transition> {
    let active = require_active(state)?;
    if at.utc_ms <= active.start_at_ms {
        return Err(DomainError::InvalidRange);
    }

    let closed = active.clone().closed(at)?;
    closed.validate()?;

    let point = TimePoint::new(ids.point_id.clone(), at, PointKind::Punch, source)
        .with_segment(closed.id.clone());

    let mut effects = vec![
        Effect::UpdateSegment(closed.clone()),
        Effect::InsertTimePoint(point),
    ];
    let mut undo = UndoToken {
        point_id: ids.point_id.clone(),
        kind: PointKind::Punch,
        issued_at_ms: at.utc_ms,
        window_ms: DEFAULT_UNDO_WINDOW_MS,
        closed_segment: Some(closed.clone()),
        opened_segment_id: None,
        edit: None,
    };

    let (next_state, pending) = match mode {
        PunchMode::Stop => (
            TimerState {
                phase: Phase::Idle,
                active: None,
                last_point_kind: Some(PointKind::Punch),
            },
            Some(closed.id.clone()),
        ),
        PunchMode::Chain => {
            let next = Segment::new_active(ids.segment_id.clone(), at);
            next.validate()?;
            effects.push(Effect::InsertSegment(next.clone()));
            undo.opened_segment_id = Some(next.id.clone());
            (
                TimerState {
                    phase: Phase::Recording,
                    active: Some(next),
                    last_point_kind: Some(PointKind::Punch),
                },
                Some(closed.id.clone()),
            )
        }
    };

    Ok(Transition {
        state: next_state,
        effects,
        pending_note_segment_id: pending,
        undo: Some(undo),
    })
}

/// T6：暂停 = 关闭当前段并写 `pause` 事实，之后处于 Idle。
pub fn pause(
    state: &TimerState,
    at: Instant,
    source: PointSource,
    ids: NewIds,
) -> DomainResult<Transition> {
    close_with(state, at, source, ids.point_id, PointKind::Pause, None)
}

/// T5：当天结束，关闭当前段并写 `finish` 事实；仍允许补记。
pub fn finish(
    state: &TimerState,
    at: Instant,
    source: PointSource,
    ids: NewIds,
) -> DomainResult<Transition> {
    let active = require_active(state)?;
    let pending = Some(active.id.clone());
    close_with(state, at, source, ids.point_id, PointKind::Finish, pending)
}

/// T9：撤销最近一次事实（15 秒窗口）。
pub fn undo(
    state: &TimerState,
    at: Instant,
    token: &UndoToken,
    point_id: TimePointId,
    source: PointSource,
) -> DomainResult<Transition> {
    if token.expired(at.utc_ms) {
        return Err(DomainError::UndoExpired);
    }

    let point = TimePoint::new(point_id, at, PointKind::Undo, source)
        .with_payload(format!("undo_of={}", token.point_id));

    match token.kind {
        PointKind::Start | PointKind::Resume => {
            // 删除刚创建的空 active 段，回到 Idle。
            let active = require_active(state)?;
            if token.opened_segment_id.as_deref() != Some(active.id.as_str()) || !is_blank(active) {
                return Err(DomainError::UndoUnavailable);
            }
            Ok(Transition {
                state: TimerState {
                    phase: Phase::Idle,
                    active: None,
                    last_point_kind: Some(PointKind::Undo),
                },
                effects: vec![
                    Effect::MarkPointUndone {
                        id: token.point_id.clone(),
                        at_ms: at.utc_ms,
                    },
                    Effect::SoftDeleteSegment {
                        id: active.id.clone(),
                        at_ms: at.utc_ms,
                    },
                    Effect::InsertTimePoint(point.with_segment(active.id.clone())),
                ],
                pending_note_segment_id: None,
                undo: None,
            })
        }
        PointKind::Punch | PointKind::Pause | PointKind::Finish => {
            let closed = token
                .closed_segment
                .clone()
                .ok_or(DomainError::UndoUnavailable)?;
            let reopened = closed.reopened();
            let mut effects = vec![Effect::MarkPointUndone {
                id: token.point_id.clone(),
                at_ms: at.utc_ms,
            }];

            // 链式打点：软删除打点瞬间创建的下一段；若已填写内容则拒绝。
            if let Some(next_id) = &token.opened_segment_id {
                let next = state.active.as_ref().ok_or(DomainError::UndoUnavailable)?;
                if &next.id != next_id || !is_blank(next) {
                    return Err(DomainError::UndoUnavailable);
                }
                effects.push(Effect::SoftDeleteSegment {
                    id: next_id.clone(),
                    at_ms: at.utc_ms,
                });
            }

            effects.push(Effect::UpdateSegment(reopened.clone()));
            effects.push(Effect::InsertTimePoint(
                point.with_segment(reopened.id.clone()),
            ));

            Ok(Transition {
                state: TimerState {
                    phase: Phase::Recording,
                    active: Some(reopened),
                    last_point_kind: Some(PointKind::Undo),
                },
                effects,
                pending_note_segment_id: None,
                undo: None,
            })
        }
        PointKind::Edit | PointKind::Backfill | PointKind::Delete => {
            let edit = token.edit.clone().ok_or(DomainError::UndoUnavailable)?;

            // 恢复一个 active 段时，不能与其它正在进行的段冲突（唯一 active 不变量）。
            let restored_active = edit.restore.iter().find(|segment| segment.is_active());
            if let Some(restored) = restored_active {
                if state.active.as_ref().is_some_and(|current| {
                    current.id != restored.id && !edit.remove_ids.contains(&current.id)
                }) {
                    return Err(DomainError::UndoUnavailable);
                }
            }

            let mut effects = vec![Effect::MarkPointUndone {
                id: token.point_id.clone(),
                at_ms: at.utc_ms,
            }];
            for id in &edit.remove_ids {
                effects.push(Effect::SoftDeleteSegment {
                    id: id.clone(),
                    at_ms: at.utc_ms,
                });
            }
            for segment in &edit.restore {
                let mut restored = segment.clone();
                restored.revision += 1;
                effects.push(Effect::RestoreSegment(restored));
            }
            effects.push(Effect::InsertTimePoint(point));

            // 状态推导：恢复出 active 段则回到记录中；删掉了当前 active 则回空闲；
            // 其余编辑撤销不改变计时状态。
            let removes_active = state
                .active
                .as_ref()
                .is_some_and(|active| edit.remove_ids.contains(&active.id));
            let next_state = if let Some(active) = restored_active {
                TimerState {
                    phase: Phase::Recording,
                    active: Some(active.clone()),
                    last_point_kind: Some(PointKind::Undo),
                }
            } else if removes_active {
                TimerState {
                    phase: Phase::Idle,
                    active: None,
                    last_point_kind: Some(PointKind::Undo),
                }
            } else {
                TimerState {
                    last_point_kind: Some(PointKind::Undo),
                    ..state.clone()
                }
            };

            Ok(Transition {
                state: next_state,
                effects,
                pending_note_segment_id: None,
                undo: None,
            })
        }
        _ => Err(DomainError::UndoUnavailable),
    }
}

// ---- 内部辅助 ----

type Transition = crate::model::Transition;

fn require_active(state: &TimerState) -> DomainResult<&Segment> {
    if state.phase != Phase::Recording {
        return Err(DomainError::NotRecording);
    }
    state.active.as_ref().ok_or(DomainError::NotRecording)
}

fn is_blank(segment: &Segment) -> bool {
    segment.title.is_empty() && segment.note.is_empty() && segment.tags.is_empty()
}

fn open_segment(
    at: Instant,
    source: PointSource,
    ids: NewIds,
    kind: PointKind,
) -> DomainResult<Transition> {
    let segment = Segment::new_active(ids.segment_id, at);
    segment.validate()?;
    let point =
        TimePoint::new(ids.point_id.clone(), at, kind, source).with_segment(segment.id.clone());

    Ok(Transition {
        state: TimerState {
            phase: Phase::Recording,
            active: Some(segment.clone()),
            last_point_kind: Some(kind),
        },
        effects: vec![
            Effect::InsertSegment(segment.clone()),
            Effect::InsertTimePoint(point),
        ],
        pending_note_segment_id: None,
        undo: Some(UndoToken {
            point_id: ids.point_id,
            kind,
            issued_at_ms: at.utc_ms,
            window_ms: DEFAULT_UNDO_WINDOW_MS,
            closed_segment: None,
            opened_segment_id: Some(segment.id.clone()),
            edit: None,
        }),
    })
}

fn close_with(
    state: &TimerState,
    at: Instant,
    source: PointSource,
    point_id: TimePointId,
    kind: PointKind,
    pending_note_segment_id: Option<SegmentId>,
) -> DomainResult<Transition> {
    let active = require_active(state)?;
    if at.utc_ms <= active.start_at_ms {
        return Err(DomainError::InvalidRange);
    }

    let closed = active.clone().closed(at)?;
    closed.validate()?;

    let point = TimePoint::new(point_id.clone(), at, kind, source).with_segment(closed.id.clone());

    Ok(Transition {
        state: TimerState {
            phase: Phase::Idle,
            active: None,
            last_point_kind: Some(kind),
        },
        effects: vec![
            Effect::UpdateSegment(closed.clone()),
            Effect::InsertTimePoint(point),
        ],
        pending_note_segment_id,
        undo: Some(UndoToken {
            point_id,
            kind,
            issued_at_ms: at.utc_ms,
            window_ms: DEFAULT_UNDO_WINDOW_MS,
            closed_segment: Some(closed),
            opened_segment_id: None,
            edit: None,
        }),
    })
}
