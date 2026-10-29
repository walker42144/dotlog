//! 时间线编辑与补录规则（`docs/domain-model.md` §9）。
//!
//! 纯函数：冲突段由调用方（application）先查询传入，本模块只做校验与变更生成。

use crate::error::{DomainError, DomainResult};
use crate::model::{
    EditUndo, Effect, Instant, NewIds, Phase, PointKind, PointSource, Segment, SegmentId,
    TimePoint, TimePointId, TimerState, Transition, UndoToken, DEFAULT_UNDO_WINDOW_MS,
};

/// 补录跨度上限：24 小时（domain-model §9）。
const MAX_BACKILL_MS: i64 = 86_400_000;

/// 补录入参（domain-model §9）。
pub struct BackfillParams {
    pub start_ms: i64,
    pub end_ms: i64,
    pub title: String,
    pub note: String,
    pub at: Instant,
    pub ids: NewIds,
    pub source: PointSource,
}

/// T11：补录一条已关闭段。`overlapping` 是与 `[start_ms, end_ms)` 相交的全部非删除段。
pub fn backfill(
    state: &TimerState,
    overlapping: &[Segment],
    params: BackfillParams,
) -> DomainResult<Transition> {
    let BackfillParams {
        start_ms,
        end_ms,
        title,
        note,
        at,
        ids,
        source,
    } = params;
    if end_ms <= start_ms {
        return Err(DomainError::InvalidRange);
    }
    if end_ms - start_ms > MAX_BACKILL_MS {
        return Err(DomainError::BackfillTooLong);
    }
    if end_ms > at.utc_ms {
        return Err(DomainError::FutureTime);
    }
    if !overlapping.is_empty() {
        return Err(DomainError::Overlap(overlapping.to_vec()));
    }

    let segment = Segment {
        id: ids.segment_id.clone(),
        start_at_ms: start_ms,
        end_at_ms: Some(end_ms),
        title: title.trim().to_string(),
        note: note.trim().to_string(),
        tags: Vec::new(),
        category_id: None,
        is_focus: false,
        is_ignored: false,
        tz_offset_min: at.tz_offset_min,
        revision: 1,
    };
    segment.validate()?;

    let edit = EditUndo {
        restore: Vec::new(),
        remove_ids: vec![ids.segment_id.clone()],
    };
    let point = TimePoint::new(ids.point_id.clone(), at, PointKind::Backfill, source)
        .with_segment(segment.id.clone())
        .with_payload(edit.to_payload());

    Ok(Transition {
        state: state.clone(),
        effects: vec![Effect::InsertSegment(segment), Effect::InsertTimePoint(point)],
        pending_note_segment_id: None,
        undo: Some(UndoToken::edit(ids.point_id, PointKind::Backfill, at, edit)),
    })
}

/// 修改段的起点 / 终点（拖拽边界或表单输入）。`conflicts` 为与新区间相交的其它段。
///
/// 规则（domain-model §9）：起点必须早于 now 且早于终点；终点必须晚于起点；
/// active 段不可设终点；不与任何其它段重叠。
pub fn resize(
    segment: &Segment,
    conflicts: &[Segment],
    new_start_ms: i64,
    new_end_ms: Option<i64>,
    at: Instant,
) -> DomainResult<Segment> {
    if new_start_ms >= at.utc_ms {
        return Err(DomainError::FutureTime);
    }
    match new_end_ms {
        Some(end) => {
            if segment.is_active() {
                return Err(DomainError::ActiveHasNoEnd);
            }
            if end <= new_start_ms {
                return Err(DomainError::InvalidRange);
            }
            if end > at.utc_ms {
                return Err(DomainError::FutureTime);
            }
        }
        None => {
            if !segment.is_active() {
                // 已关闭段不能只改起点而把终点清空。
                return Err(DomainError::InvalidRange);
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(DomainError::Overlap(conflicts.to_vec()));
    }

    let mut updated = segment.clone();
    updated.start_at_ms = new_start_ms;
    updated.end_at_ms = new_end_ms;
    updated.revision += 1;
    updated.validate()?;
    Ok(updated)
}

/// 在 `split_at_ms` 处把一段拆成两段：前半保留原 id，后半为新段（继承标题/备注/标签）。
/// 拆分点必须严格落在段内；active 段拆分后后半仍为 active。
pub fn split(
    segment: &Segment,
    split_at_ms: i64,
    new_segment_id: SegmentId,
    now_ms: i64,
) -> DomainResult<(Segment, Segment)> {
    let end = segment.end_at_ms.unwrap_or(now_ms);
    if split_at_ms <= segment.start_at_ms || split_at_ms >= end {
        return Err(DomainError::InvalidRange);
    }

    let mut first = segment.clone();
    first.end_at_ms = Some(split_at_ms);
    first.revision += 1;

    let second = Segment {
        id: new_segment_id,
        start_at_ms: split_at_ms,
        end_at_ms: segment.end_at_ms,
        title: segment.title.clone(),
        note: segment.note.clone(),
        tags: segment.tags.clone(),
        category_id: segment.category_id.clone(),
        is_focus: segment.is_focus,
        is_ignored: segment.is_ignored,
        tz_offset_min: segment.tz_offset_min,
        revision: 1,
    };
    first.validate()?;
    second.validate()?;
    Ok((first, second))
}

/// 合并连续段，中间空档一并覆盖（ipc-contract §3.2）。输入至少两段且均非 active。
pub fn merge(mut segments: Vec<Segment>, merged_id: SegmentId) -> DomainResult<Segment> {
    if segments.len() < 2 {
        return Err(DomainError::MergeNeedsTwo);
    }
    if segments.iter().any(Segment::is_active) {
        return Err(DomainError::MergeActive);
    }
    segments.sort_by_key(|segment| segment.start_at_ms);

    let first = segments.first().expect("len >= 2");
    let last = segments.last().expect("len >= 2");

    let mut titles: Vec<&str> = Vec::new();
    let mut notes: Vec<&str> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    for segment in &segments {
        if !segment.title.is_empty() && !titles.contains(&segment.title.as_str()) {
            titles.push(segment.title.as_str());
        }
        if !segment.note.is_empty() && !notes.contains(&segment.note.as_str()) {
            notes.push(segment.note.as_str());
        }
        for tag in &segment.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }

    let merged = Segment {
        id: merged_id,
        start_at_ms: first.start_at_ms,
        end_at_ms: Some(last.end_at_ms.expect("merge 不含 active 段")),
        title: titles.join("、"),
        note: notes.join("\n"),
        tags,
        // 分类取最早一段；专注取并集，忽略取交集（全部忽略才算忽略）。
        category_id: segments
            .iter()
            .find_map(|segment| segment.category_id.clone()),
        is_focus: segments.iter().any(|segment| segment.is_focus),
        is_ignored: segments.iter().all(|segment| segment.is_ignored),
        tz_offset_min: first.tz_offset_min,
        revision: 1,
    };
    merged.validate()?;
    Ok(merged)
}

/// 删除段（软删除）。active 段先以 now 关闭再删除，写两条事实（domain-model §9）。
pub fn delete(
    state: &TimerState,
    segment: &Segment,
    at: Instant,
    source: PointSource,
    delete_point_id: TimePointId,
    finish_point_id: TimePointId,
) -> DomainResult<Transition> {
    let mut effects = Vec::new();
    let mut next_active = state.active.clone();

    if segment.is_active() {
        let closed = segment.clone().closed(at)?;
        effects.push(Effect::UpdateSegment(closed.clone()));
        effects.push(Effect::InsertTimePoint(
            TimePoint::new(finish_point_id, at, PointKind::Finish, source)
                .with_segment(closed.id.clone()),
        ));
        next_active = None;
    }

    let edit = EditUndo {
        restore: vec![segment.clone()],
        remove_ids: Vec::new(),
    };
    effects.push(Effect::SoftDeleteSegment {
        id: segment.id.clone(),
        at_ms: at.utc_ms,
    });
    effects.push(Effect::InsertTimePoint(
        TimePoint::new(delete_point_id.clone(), at, PointKind::Delete, source)
            .with_segment(segment.id.clone())
            .with_payload(edit.to_payload()),
    ));

    let phase = if next_active.is_some() {
        Phase::Recording
    } else {
        Phase::Idle
    };

    Ok(Transition {
        state: TimerState {
            phase,
            active: next_active,
            last_point_kind: Some(PointKind::Delete),
        },
        effects,
        pending_note_segment_id: None,
        undo: Some(UndoToken::edit(delete_point_id, PointKind::Delete, at, edit)),
    })
}

impl UndoToken {
    /// 编辑类操作的撤销凭据（backfill / delete 由本模块产生；split / merge / resize
    /// 在 application 组装，结构相同）。
    pub fn edit(point_id: TimePointId, kind: PointKind, at: Instant, edit: EditUndo) -> Self {
        Self {
            point_id,
            kind,
            issued_at_ms: at.utc_ms,
            window_ms: DEFAULT_UNDO_WINDOW_MS,
            closed_segment: None,
            opened_segment_id: None,
            edit: Some(edit),
        }
    }
}
