use std::collections::HashMap;
use std::sync::Mutex;

use dotlog_domain::{
    backfill, clip, day_window, delete as delete_op, finish, gaps as day_gaps, local_date, merge,
    parse_day_window, pause, punch, resize, resume, split, start, undo as undo_last,
    BackfillParams, EditUndo, Effect, Instant, NewIds, Phase, PointKind, PointSource, PunchMode,
    Segment, TimePoint, TimerState, Transition, UndoToken, DEFAULT_UNDO_WINDOW_MS,
};

use crate::dto::{
    closed_duration_seconds, pending_note, BackfillInput, CategoryView, DayStatsView, DayView,
    GapView, HistoryView, PunchResultView, SegmentPatch, SegmentView, TimerStateView,
    UndoResultView,
};
use crate::error::{AppError, AppResult};
use crate::ports::{Clock, TimerStore};

/// 短段阈值：关闭段不足 15 秒时 UI 提示补记（`punch.short_segment_ms` 默认值）。
const SHORT_SEGMENT_MS: i64 = 15_000;

/// 计时用例。持有持久化端口与时钟，状态机权威始终在 `dotlog-domain`。
pub struct TimerService<S: TimerStore, C: Clock> {
    pub(crate) store: S,
    pub(crate) clock: C,
    mode: PunchMode,
    pub(crate) operation: Mutex<()>,
    pub(crate) link_live: Mutex<Option<crate::tomatodo_link::LinkState>>,
}

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    pub fn new(store: S, clock: C, mode: PunchMode) -> Self {
        Self {
            store,
            clock,
            mode,
            operation: Mutex::new(()),
            link_live: Mutex::new(None),
        }
    }

    /// `timer_get_state`：读取当前相位与进行中的段。
    pub fn get_state(&self) -> AppResult<TimerStateView> {
        let state = self.load_state()?;
        Ok(self.view(&state))
    }

    pub fn pending_note(&self) -> AppResult<Option<SegmentView>> {
        let Some(segment) = self.store.pending_note_segment()? else {
            return Ok(None);
        };
        let now = self.clock.now().utc_ms;
        let slices = self.store.session_segments(&segment.id)?;
        let mut view = self.view_of(&segment, now);
        if let Some(first) = slices.first() {
            view.start_at = first.start_at_ms;
        }
        view.duration_seconds = slices.iter().map(|s| s.duration_ms(now)).sum::<i64>() / 1000;
        Ok(Some(view))
    }

    pub fn confirm_session(&self, id: &str) -> AppResult<()> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.store.confirm_session(id, self.clock.now().utc_ms)
    }

    /// `timer_start`：空闲时开始；暂停中时等同继续（T2）。
    pub fn start(&self, source: PointSource) -> AppResult<TimerStateView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.load_state()?;
        let at = self.clock.now();
        let mut transition = if state.is_paused() {
            resume(&state, at, source, new_ids())?
        } else {
            start(&state, at, source, new_ids())?
        };
        if state.is_paused() {
            if let Some(previous) = self.store.current_session_segments()?.last() {
                for effect in &mut transition.effects {
                    if let Effect::InsertSegment(s) = effect {
                        s.title = previous.title.clone();
                        s.note = previous.note.clone();
                        s.tags = previous.tags.clone();
                        s.category_id = previous.category_id.clone();
                        s.is_focus = previous.is_focus;
                        s.is_ignored = previous.is_ignored;
                        transition.state.active = Some(s.clone());
                    }
                }
            }
        }
        self.commit(&transition)?;
        Ok(self.view(&transition.state))
    }

    /// `timer_punch`：按配置的 stop / chain 模式切断当前段。
    pub fn punch(&self, source: PointSource) -> AppResult<PunchResultView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.load_state()?;
        let at = self.clock.now();
        let transition = punch(&state, at, self.configured_mode(), source, new_ids())?;
        self.commit(&transition)?;
        self.punch_view(&transition, at)
    }

    /// `timer_finish`：结束当天记录。
    pub fn finish(&self, source: PointSource) -> AppResult<PunchResultView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.load_state()?;
        let at = self.clock.now();
        if state.is_paused() {
            let slices = self.store.current_session_segments()?;
            let segment = slices
                .last()
                .cloned()
                .or_else(|| {
                    self.store
                        .last_point()
                        .ok()
                        .flatten()
                        .and_then(|p| p.segment_id)
                        .and_then(|id| self.store.find_segment(&id).ok().flatten())
                })
                .ok_or_else(|| unavailable("没有可结束的暂停记录"))?;
            let point = TimePoint::new(crate::new_id(), at, PointKind::Finish, source)
                .with_segment(segment.id.clone())
                .with_payload(serde_json::json!({"pausedFinish":true}).to_string());
            let point_id = point.id.clone();
            self.store
                .commit(&[Effect::InsertTimePoint(point)], at.utc_ms)?;
            let mut closed = self.view_of(&segment, at.utc_ms);
            closed.duration_seconds =
                slices.iter().map(|s| s.duration_ms(at.utc_ms)).sum::<i64>() / 1000;
            if let Some(first) = slices.first() {
                closed.start_at = first.start_at_ms;
            }
            return Ok(PunchResultView {
                closed_segment: closed,
                new_segment: None,
                pending_note: pending_note(&segment),
                undo_token: Some(point_id),
                short_segment: false,
            });
        }
        let transition = finish(&state, at, source, new_ids())?;
        self.commit(&transition)?;
        self.punch_view(&transition, at)
    }

    /// `timer_pause`：关闭当前段并标记为暂停，之后可 `start` 继续。
    pub fn pause(&self, source: PointSource) -> AppResult<PunchResultView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.load_state()?;
        let at = self.clock.now();
        let transition = pause(&state, at, source, new_ids())?;
        self.commit(&transition)?;
        self.punch_view(&transition, at)
    }

    /// `timer_undo`：撤销最近一次事实；凭据由持久层最近事实重建（15 秒窗口）。
    /// 编辑类事实（edit / backfill / delete）从 payload 恢复撤销数据。
    pub fn undo(&self) -> AppResult<UndoResultView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.load_state()?;
        let at = self.clock.now();
        let point = self
            .store
            .last_point()?
            .ok_or_else(|| unavailable("没有可撤销的操作"))?;

        let paused_finish = point.kind == PointKind::Finish
            && point
                .payload
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| v.get("pausedFinish").and_then(|v| v.as_bool()))
                .unwrap_or(false);
        if paused_finish {
            if at.utc_ms - point.occurred_at_ms
                > self.setting_ms("punch.undo_window_ms", DEFAULT_UNDO_WINDOW_MS)
            {
                return Err(dotlog_domain::DomainError::UndoExpired.into());
            }
            let id = point
                .segment_id
                .ok_or_else(|| unavailable("暂停记录不存在"))?;
            let undo_point =
                TimePoint::new(crate::new_id(), at, PointKind::Undo, PointSource::Button)
                    .with_payload(serde_json::json!({"restorePausedSession":id}).to_string());
            self.store.commit(
                &[
                    Effect::MarkPointUndone {
                        id: point.id,
                        at_ms: at.utc_ms,
                    },
                    Effect::InsertTimePoint(undo_point),
                ],
                at.utc_ms,
            )?;
            return Ok(UndoResultView {
                restored_segment: None,
                removed_segment_id: None,
            });
        }

        let edit = match point.kind {
            PointKind::Start
            | PointKind::Resume
            | PointKind::Punch
            | PointKind::Pause
            | PointKind::Finish => None,
            PointKind::Edit | PointKind::Backfill | PointKind::Delete => {
                let payload = point
                    .payload
                    .as_deref()
                    .ok_or_else(|| unavailable("撤销信息不完整"))?;
                Some(EditUndo::from_payload(payload).ok_or_else(|| unavailable("撤销信息不完整"))?)
            }
            PointKind::Undo => return Err(unavailable("最近一次操作不支持撤销")),
        };

        let closed_segment = match point.kind {
            PointKind::Punch | PointKind::Pause | PointKind::Finish => {
                let id = point
                    .segment_id
                    .as_deref()
                    .ok_or_else(|| unavailable("撤销信息不完整"))?;
                let segment = self
                    .store
                    .find_segment(id)?
                    .ok_or_else(|| unavailable("这条记录已不存在"))?;
                Some(segment)
            }
            _ => None,
        };

        // 链式打点 / 开始记录会把新建段设为 active，其起点等于事实发生时刻。
        let opened_segment_id = state
            .active
            .as_ref()
            .filter(|active| active.start_at_ms == point.occurred_at_ms)
            .map(|active| active.id.clone());

        let token = UndoToken {
            point_id: point.id,
            kind: point.kind,
            issued_at_ms: point.occurred_at_ms,
            window_ms: self.setting_ms("punch.undo_window_ms", DEFAULT_UNDO_WINDOW_MS),
            closed_segment,
            opened_segment_id,
            edit,
        };

        let transition = undo_last(&state, at, &token, crate::new_id(), PointSource::Button)?;
        self.commit(&transition)?;

        Ok(UndoResultView {
            restored_segment: self
                .load_state()?
                .active
                .as_ref()
                .map(|segment| self.view_of(segment, at.utc_ms)),
            removed_segment_id: transition.effects.iter().find_map(|effect| match effect {
                Effect::SoftDeleteSegment { id, .. } => Some(id.clone()),
                _ => None,
            }),
        })
    }

    /// `day_today`：不带日期时返回今天；`day_get` 传入 `YYYY-MM-DD`。
    pub fn day(&self, date: Option<&str>) -> AppResult<DayView> {
        let now = self.clock.now();
        let policy = self.day_policy()?;
        let categories = self.category_map()?;

        let (window_start, window_end, date_label) = match date {
            Some(value) => {
                let (start, end) =
                    parse_day_window(value, now.tz_offset_min, policy.day_start_minutes)
                        .ok_or_else(invalid_date)?;
                (start, end, value.to_string())
            }
            None => {
                let (start, end) =
                    day_window(now.utc_ms, now.tz_offset_min, policy.day_start_minutes);
                let label = local_date(now.utc_ms, now.tz_offset_min, policy.day_start_minutes);
                (start, end, label)
            }
        };

        // 今天只统计到 now，未来时段不算空白。
        let stats_end = window_end.min(now.utc_ms).max(window_start);

        let segments = self.store.day_segments(window_start, window_end)?;
        let mut views = Vec::new();
        let mut clips = Vec::new();
        for segment in &segments {
            if let Some(slice) = clip(segment, window_start, stats_end, now.utc_ms) {
                let category = segment
                    .category_id
                    .as_ref()
                    .and_then(|id| categories.get(id))
                    .cloned();
                views.push(
                    SegmentView::from_slice(segment, slice.start_ms, slice.end_ms, now.utc_ms)
                        .with_category(category),
                );
                clips.push(slice);
            }
        }

        let gaps = day_gaps(&clips, window_start, stats_end)
            .into_iter()
            .map(|gap| GapView {
                start_at: gap.start_ms,
                end_at: gap.end_ms,
                seconds: gap.seconds(),
            })
            .collect();

        let aggregate =
            dotlog_domain::stats_bucket(&segments, window_start, stats_end, now.utc_ms, policy);
        let day_totals = dotlog_domain::DayTotals {
            recorded_seconds: aggregate.recorded_seconds,
            effective_seconds: aggregate.effective_seconds,
            blank_seconds: aggregate.blank_seconds,
            focus_seconds: aggregate.focus_seconds,
            switch_count: aggregate.switch_count,
            segment_count: aggregate.segment_count,
            average_segment_seconds: aggregate.average_segment_seconds,
        };
        let coverage_rate = aggregate.coverage_rate;

        Ok(DayView {
            date: date_label,
            window_start,
            window_end,
            segments: views,
            gaps,
            stats: DayStatsView::new(day_totals, coverage_rate),
        })
    }

    /// `segment_update`：补记标题 / 备注 / 起止时间，乐观锁校验 `revision`。
    /// 修改时间时做冲突预检，重叠返回 `E_OVERLAP`（携带冲突段列表）。
    pub fn update_segment(
        &self,
        id: &str,
        revision: u32,
        patch: SegmentPatch,
    ) -> AppResult<SegmentView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let at = self.clock.now();
        let segment = self
            .store
            .find_segment(id)?
            .ok_or_else(|| AppError::store("E_SEGMENT_NOT_FOUND", "这条记录不存在"))?;

        if segment.revision != revision {
            return Err(AppError::store(
                "E_STALE_REVISION",
                "记录已被修改，请刷新后重试",
            ));
        }

        let mut updated = segment.clone();
        if let Some(title) = patch.title {
            updated.title = title.trim().to_string();
        }
        if let Some(note) = patch.note {
            updated.note = note.trim().to_string();
        }
        if let Some(category_id) = patch.category_id {
            updated.category_id = category_id
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty());
        }
        if let Some(tags) = patch.tags {
            updated.tags = tags
                .into_iter()
                .map(|s| s.trim().trim_start_matches('#').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            updated.tags.sort();
            updated.tags.dedup();
            if updated.tags.len() > 30 || updated.tags.iter().any(|t| t.chars().count() > 40) {
                return Err(AppError::store(
                    "E_INVALID_RANGE",
                    "标签最多30个，每个最多40字",
                ));
            }
        }
        if let Some(is_focus) = patch.is_focus {
            updated.is_focus = is_focus;
        }
        if let Some(is_ignored) = patch.is_ignored {
            updated.is_ignored = is_ignored;
        }

        // 时间字段任一出现即走编辑规则（domain-model §9）。
        if patch.start_at.is_some() || patch.end_at.is_some() {
            let new_start = patch.start_at.unwrap_or(segment.start_at_ms);
            let new_end = patch.end_at.or(segment.end_at_ms);
            let conflicts = self.conflicts_for(id, new_start, new_end.unwrap_or(at.utc_ms))?;
            updated = resize(&updated, &conflicts, new_start, new_end, at)?;
        } else {
            updated.revision += 1;
        }

        let mut originals = vec![segment];
        let mut effects = vec![Effect::UpdateSegment(updated.clone())];
        for sibling in self.store.session_segments(id)? {
            if sibling.id == id {
                continue;
            }
            let mut revised = sibling.clone();
            revised.title = updated.title.clone();
            revised.note = updated.note.clone();
            revised.tags = updated.tags.clone();
            revised.category_id = updated.category_id.clone();
            revised.is_focus = updated.is_focus;
            revised.is_ignored = updated.is_ignored;
            revised.revision += 1;
            originals.push(sibling);
            effects.push(Effect::UpdateSegment(revised));
        }
        let undo_payload = EditUndo {
            restore: originals,
            remove_ids: Vec::new(),
        };
        let point = TimePoint::new(crate::new_id(), at, PointKind::Edit, PointSource::Button)
            .with_segment(updated.id.clone())
            .with_payload(undo_payload.to_payload());
        effects.push(Effect::InsertTimePoint(point));
        self.store.commit(&effects, at.utc_ms)?;

        Ok(
            SegmentView::from_segment(&updated, at.utc_ms)
                .with_category(self.category_of(&updated)),
        )
    }

    /// `segment_backfill`：创建已关闭段；与已有段重叠则拒绝并返回冲突列表。
    pub fn backfill_segment(&self, input: BackfillInput) -> AppResult<SegmentView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let at = self.clock.now();
        let state = self.load_state()?;
        let overlapping = self.store.day_segments(input.start_at, input.end_at)?;
        let transition = backfill(
            &state,
            &overlapping,
            BackfillParams {
                start_ms: input.start_at,
                end_ms: input.end_at,
                title: input.title.unwrap_or_default(),
                note: input.note.unwrap_or_default(),
                at,
                ids: new_ids(),
                source: PointSource::Button,
            },
        )?;

        // domain 的补录只处理时间与文本；分类 / 专注 / 忽略由应用层补上。
        let mut transition = transition;
        for effect in &mut transition.effects {
            if let Effect::InsertSegment(segment) = effect {
                segment.category_id = input.category_id.clone();
                segment.is_focus = input.is_focus;
                segment.is_ignored = input.is_ignored;
            }
        }
        self.commit(&transition)?;
        let segment = transition
            .effects
            .iter()
            .find_map(|effect| match effect {
                Effect::InsertSegment(segment) => Some(segment.clone()),
                _ => None,
            })
            .expect("backfill 必产生 InsertSegment");
        Ok(
            SegmentView::from_segment(&segment, at.utc_ms)
                .with_category(self.category_of(&segment)),
        )
    }

    /// `segment_delete`：软删除；active 段先以 now 关闭再删除。
    pub fn delete_segment(&self, id: &str) -> AppResult<()> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let at = self.clock.now();
        let state = self.load_state()?;
        let segment = self
            .store
            .find_segment(id)?
            .ok_or_else(|| AppError::store("E_SEGMENT_NOT_FOUND", "这条记录不存在"))?;
        let transition = delete_op(
            &state,
            &segment,
            at,
            PointSource::Button,
            crate::new_id(),
            crate::new_id(),
        )?;
        self.commit_link_closure(&transition.effects, Some(id), false, at)
    }

    /// `segment_split`：在指定时刻拆成两段，返回 `[前半, 后半]`。
    pub fn split_segment(&self, id: &str, at_ms: i64) -> AppResult<Vec<SegmentView>> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .link_context()?
            .pending_notes
            .iter()
            .any(|pending| pending == id)
        {
            return Err(AppError::store(
                "E_PENDING_NOTE",
                "请先在番茄联动面板确认备注，再拆分记录",
            ));
        }
        let at = self.clock.now();
        let segment = self
            .store
            .find_segment(id)?
            .ok_or_else(|| AppError::store("E_SEGMENT_NOT_FOUND", "这条记录不存在"))?;

        let second_id = crate::new_id();
        let (first, second) = split(&segment, at_ms, second_id.clone(), at.utc_ms)?;

        let undo_payload = EditUndo {
            restore: vec![segment],
            remove_ids: vec![second_id],
        };
        let point = TimePoint::new(crate::new_id(), at, PointKind::Edit, PointSource::Button)
            .with_segment(first.id.clone())
            .with_payload(undo_payload.to_payload());
        self.store.commit(
            &[
                Effect::UpdateSegment(first.clone()),
                Effect::InsertSegment(second.clone()),
                Effect::InsertTimePoint(point),
            ],
            at.utc_ms,
        )?;

        Ok(vec![
            SegmentView::from_segment(&first, at.utc_ms).with_category(self.category_of(&first)),
            SegmentView::from_segment(&second, at.utc_ms).with_category(self.category_of(&second)),
        ])
    }

    /// `segment_merge`：合并连续段，中间空档一并覆盖；返回合并后的段。
    pub fn merge_segments(&self, ids: &[String]) -> AppResult<SegmentView> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .link_context()?
            .pending_notes
            .iter()
            .any(|pending| ids.contains(pending))
        {
            return Err(AppError::store(
                "E_PENDING_NOTE",
                "请先在番茄联动面板确认备注，再合并记录",
            ));
        }
        let at = self.clock.now();
        let mut segments = Vec::with_capacity(ids.len());
        for id in ids {
            segments.push(
                self.store
                    .find_segment(id)?
                    .ok_or_else(|| AppError::store("E_SEGMENT_NOT_FOUND", "这条记录不存在"))?,
            );
        }

        let unique: std::collections::HashSet<_> = ids.iter().collect();
        if unique.len() != ids.len() {
            return Err(AppError::store("E_INVALID_RANGE", "不能重复选择同一段"));
        }
        let merged = merge(segments.clone(), crate::new_id())?;
        let conflicts: Vec<_> = self
            .store
            .day_segments(merged.start_at_ms, merged.end_at_ms.unwrap())?
            .into_iter()
            .filter(|s| !ids.contains(&s.id))
            .collect();
        if !conflicts.is_empty() {
            return Err(dotlog_domain::DomainError::Overlap(conflicts).into());
        }

        let undo_payload = EditUndo {
            restore: segments,
            remove_ids: vec![merged.id.clone()],
        };
        let point = TimePoint::new(crate::new_id(), at, PointKind::Edit, PointSource::Button)
            .with_segment(merged.id.clone())
            .with_payload(undo_payload.to_payload());

        let mut effects: Vec<Effect> = Vec::new();
        for segment in &undo_payload.restore {
            effects.push(Effect::SoftDeleteSegment {
                id: segment.id.clone(),
                at_ms: at.utc_ms,
            });
        }
        effects.push(Effect::InsertSegment(merged.clone()));
        effects.push(Effect::InsertTimePoint(point));
        self.store.commit(&effects, at.utc_ms)?;

        Ok(SegmentView::from_segment(&merged, at.utc_ms).with_category(self.category_of(&merged)))
    }

    /// `history_query`：倒序段列表 + 文本搜索（历史页）。
    pub fn history(
        &self,
        from_ms: i64,
        to_ms: i64,
        text: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> AppResult<HistoryView> {
        let now = self.clock.now().utc_ms;
        let text = text.map(str::trim).filter(|value| !value.is_empty());
        let categories = self.category_map()?;
        let (items, total) =
            self.store
                .history_segments(from_ms, to_ms, text, limit.min(200), offset)?;
        Ok(HistoryView {
            items: items
                .iter()
                .map(|segment| {
                    let category = segment
                        .category_id
                        .as_ref()
                        .and_then(|id| categories.get(id))
                        .cloned();
                    SegmentView::from_segment(segment, now).with_category(category)
                })
                .collect(),
            total,
        })
    }

    fn configured_mode(&self) -> PunchMode {
        self.store
            .settings("app")
            .ok()
            .and_then(|rows| {
                rows.into_iter()
                    .find(|(key, _)| key == "punch.mode")
                    .and_then(|(_, value)| serde_json::from_str::<PunchMode>(&value).ok())
            })
            .unwrap_or(self.mode)
    }

    fn setting_ms(&self, key: &str, default: i64) -> i64 {
        self.settings(Some("app"))
            .ok()
            .and_then(|v| v.get(key)?.as_i64())
            .unwrap_or(default)
    }

    fn category_map(&self) -> AppResult<HashMap<String, CategoryView>> {
        Ok(self
            .store
            .categories()?
            .into_iter()
            .map(|row| {
                let view = CategoryView {
                    id: row.id.clone(),
                    name: row.name,
                    color: row.color,
                    parent_id: row.parent_id,
                };
                (row.id, view)
            })
            .collect())
    }

    /// 单个段的分类视图（每次一条目录查询，只用于返回单段的命令）。
    pub(crate) fn category_of(&self, segment: &Segment) -> Option<CategoryView> {
        let id = segment.category_id.as_deref()?;
        self.store
            .categories()
            .ok()?
            .into_iter()
            .find(|row| row.id == id)
            .map(|row| CategoryView {
                id: row.id,
                name: row.name,
                color: row.color,
                parent_id: row.parent_id,
            })
    }

    /// 单个段的视图（含分类）。
    pub(crate) fn view_of(&self, segment: &Segment, now_ms: i64) -> SegmentView {
        SegmentView::from_segment(segment, now_ms).with_category(self.category_of(segment))
    }

    /// 与 `[start_ms, end_ms)` 相交的其它段（排除自身），供冲突预检。
    fn conflicts_for(
        &self,
        exclude_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> AppResult<Vec<Segment>> {
        let mut conflicts = self.store.day_segments(start_ms, end_ms)?;
        conflicts.retain(|segment| segment.id != exclude_id);
        Ok(conflicts)
    }

    pub(crate) fn load_state(&self) -> AppResult<TimerState> {
        let snapshot = self.store.snapshot()?;
        Ok(TimerState {
            phase: if snapshot.active.is_some() {
                Phase::Recording
            } else {
                Phase::Idle
            },
            active: snapshot.active,
            last_point_kind: snapshot.last_point_kind,
        })
    }

    /// 事务边界：领域层产出的 effects 必须一次性提交。
    fn commit(&self, transition: &Transition) -> AppResult<()> {
        self.store
            .commit(&transition.effects, self.clock.now().utc_ms)
    }

    pub(crate) fn view(&self, state: &TimerState) -> TimerStateView {
        let now = self.clock.now();
        let slices = self.store.current_session_segments().unwrap_or_default();
        let elapsed_seconds = if slices.is_empty() {
            state
                .active
                .as_ref()
                .map(|s| s.duration_ms(now.utc_ms) / 1000)
                .unwrap_or(0)
        } else {
            slices
                .iter()
                .map(|s| s.duration_ms(now.utc_ms))
                .sum::<i64>()
                / 1000
        };
        let session_started_at = slices
            .first()
            .map(|s| s.start_at_ms)
            .or_else(|| state.active.as_ref().map(|s| s.start_at_ms));

        TimerStateView {
            phase: state.phase,
            active: state
                .active
                .as_ref()
                .map(|segment| self.view_of(segment, now.utc_ms)),
            elapsed_seconds,
            session_started_at,
            mode: self.configured_mode(),
            paused: state.is_paused(),
        }
    }

    fn punch_view(&self, transition: &Transition, at: Instant) -> AppResult<PunchResultView> {
        let closed = transition
            .undo
            .as_ref()
            .and_then(|token| token.closed_segment.clone())
            .ok_or_else(|| AppError::store("E_UNKNOWN", "打点结果缺少被关闭的段"))?;

        let mut closed_view = self.view_of(&closed, at.utc_ms);
        let slices = self.store.session_segments(&closed.id)?;
        if !slices.is_empty() {
            closed_view.duration_seconds =
                slices.iter().map(|s| s.duration_ms(at.utc_ms)).sum::<i64>() / 1000;
            closed_view.start_at = slices.first().unwrap().start_at_ms;
        }
        Ok(PunchResultView {
            closed_segment: closed_view,
            new_segment: transition
                .state
                .active
                .as_ref()
                .map(|segment| self.view_of(segment, at.utc_ms)),
            pending_note: transition
                .pending_note_segment_id
                .as_ref()
                .and_then(|_| pending_note(&closed)),
            undo_token: transition.undo.as_ref().map(|token| token.point_id.clone()),
            short_segment: closed_duration_seconds(&closed) * 1_000
                < self.setting_ms("punch.short_segment_ms", SHORT_SEGMENT_MS),
        })
    }
}

/// 持久层分配 UUIDv7；领域层只消费。
fn new_ids() -> NewIds {
    NewIds::new(crate::new_id(), crate::new_id())
}

fn unavailable(message: &str) -> AppError {
    AppError::store("E_UNDO_UNAVAILABLE", message)
}

fn invalid_date() -> AppError {
    AppError::store("E_INVALID_RANGE", "日期格式应为 YYYY-MM-DD")
}
