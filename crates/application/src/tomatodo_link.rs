//! Stage-two use cases only: no observer worker, IPC registration or production activation.
//! Caller must route linked controls here; existing manual timer APIs remain unchanged.
use crate::{AppError, AppResult, Clock, TimerService, TimerStore};
use dotlog_domain::{Effect, Instant, PointKind, PointSource, Segment, TimePoint};
use serde::{Deserialize, Serialize};

pub const RUNTIME_KEY: &str = "tomatodo.link.v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcePhase {
    #[default]
    Unknown,
    Idle,
    Focus,
    Paused,
    Rest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSession {
    pub key: String,
    pub title: String,
    pub started_at_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub phase: SourcePhase,
    pub session: Option<SourceSession>,
    pub observed_at_ms: i64,
    pub count_up: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkState {
    pub initialized: bool,
    pub connected: bool,
    pub source_phase: SourcePhase,
    pub session: Option<SourceSession>,
    pub active_segment_id: Option<String>,
    pub manually_stopped: bool,
    pub resume_on_focus: bool,
    pub pending_notes: Vec<String>,
    /// Reject completed/older source sessions even after application restart.
    pub latest_started_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub gap_detected: bool,
    pub conflict: Option<String>,
}

#[derive(Clone, Debug)]
pub enum LinkCommand {
    Observe(Observation),
    /// Automatic desktop attachment records from now, never from an unseen past.
    ObserveAutomatic(Observation),
    EndSegment,
    StartNext,
    /// Explicit user-chosen boundary; never infer an end across a disconnected gap.
    Reconcile {
        end_at_ms: i64,
    },
    Detach,
    SaveNote {
        segment_id: String,
        revision: u32,
        note: String,
    },
    SkipNote {
        segment_id: String,
        revision: u32,
    },
}

fn error(message: &str) -> AppError {
    AppError::store("E_TOMATODO_LINK", message)
}

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    pub fn tomatodo_link_state(&self) -> AppResult<LinkState> {
        let _lock = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.link_context()
    }

    pub(crate) fn link_context(&self) -> AppResult<LinkState> {
        let mut live = self.link_live.lock().unwrap_or_else(|e| e.into_inner());
        if live.is_none() {
            *live = Some(read_context(&self.store)?);
        }
        Ok(live.as_ref().expect("initialized above").clone())
    }

    /// Caller holds operation. Keep ordinary recovery/deletion and link identity atomic.
    pub(crate) fn commit_link_closure(
        &self,
        effects: &[Effect],
        id: Option<&str>,
        keep_note: bool,
        now: Instant,
    ) -> AppResult<()> {
        let mut context = self.link_context()?;
        if id.is_none() || context.active_segment_id.as_deref() != id {
            return self.store.commit(effects, now.utc_ms);
        }
        if keep_note {
            let id = id.expect("matched active identity").to_owned();
            if !context.pending_notes.contains(&id) {
                context.pending_notes.push(id);
            }
        }
        context.active_segment_id = None;
        context.session = None;
        context.initialized = false;
        context.connected = false;
        context.source_phase = SourcePhase::Unknown;
        context.resume_on_focus = false;
        context.manually_stopped = true;
        context.conflict = None;
        let serialized = serde_json::to_string(&context).map_err(|_| error("无法保存联动状态"))?;
        self.store
            .commit_runtime(effects, RUNTIME_KEY, &serialized, now.utc_ms)?;
        *self.link_live.lock().unwrap_or_else(|e| e.into_inner()) = Some(context);
        Ok(())
    }

    /// Use the same service-level mutex as manual timer commands; runtime state and
    /// segment/time-point changes are committed together by the storage port.
    pub fn tomatodo_link(&self, command: LinkCommand) -> AppResult<LinkState> {
        let _lock = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.clock.now();
        let mut context = self.link_context()?;
        let before = context.clone();
        let actual = self.store.snapshot()?;
        let mut effects = Vec::new();
        let auto_baseline = matches!(&command, LinkCommand::ObserveAutomatic(_));
        match command {
            LinkCommand::Observe(observation) | LinkCommand::ObserveAutomatic(observation) => {
                if now.utc_ms < context.last_observed_at_ms.saturating_sub(1_000) {
                    context.connected = false;
                    context.source_phase = SourcePhase::Unknown;
                    context.gap_detected = true;
                    context.conflict = Some("检测到系统时间回拨，请校正时间后核对当前记录".into());
                    context.last_observed_at_ms = now.utc_ms;
                    let serialized =
                        serde_json::to_string(&context).map_err(|_| error("无法保存联动状态"))?;
                    self.store
                        .commit_runtime(&[], RUNTIME_KEY, &serialized, now.utc_ms)?;
                    *self.link_live.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(context.clone());
                    return Ok(context);
                }
                validate_observation(&observation, &context, now.utc_ms)?;
                let was_initialized = context.initialized;
                let timely = observation
                    .observed_at_ms
                    .saturating_sub(context.last_observed_at_ms)
                    <= 5_000;
                let was_connected = context.connected && timely;
                if context.initialized && !timely && context.active_segment_id.is_some() {
                    context.gap_detected = true;
                }
                if let Some(id) = context.active_segment_id.as_deref() {
                    if actual.active.as_ref().map(|segment| segment.id.as_str()) != Some(id) {
                        return Err(error("关联段已被其它操作改变，需要核对"));
                    }
                }
                if observation.phase != SourcePhase::Unknown {
                    context.initialized = true;
                }
                context.last_observed_at_ms = observation.observed_at_ms;
                if observation.phase == SourcePhase::Unknown {
                    context.connected = false;
                    context.gap_detected |= context.active_segment_id.is_some();
                } else {
                    let active_source =
                        matches!(observation.phase, SourcePhase::Focus | SourcePhase::Paused);
                    if active_source {
                        let source = observation
                            .session
                            .as_ref()
                            .ok_or_else(|| error("专注状态缺少会话身份"))?;
                        let same = context
                            .session
                            .as_ref()
                            .is_some_and(|s| s.key == source.key);
                        if same {
                            if context
                                .session
                                .as_ref()
                                .is_some_and(|s| s.started_at_ms != source.started_at_ms)
                            {
                                return Err(error("同一会话的开始时间发生变化，需要核对"));
                            }
                            // Live title updates only affect subsequent segments, not saved records.
                            context.session = Some(source.clone());
                            if context.active_segment_id.is_some()
                                && observation.phase == SourcePhase::Paused
                            {
                                close_owned(
                                    &mut context,
                                    actual.active.as_ref(),
                                    now,
                                    &mut effects,
                                )?;
                                context.resume_on_focus = true;
                            } else if context.resume_on_focus
                                && observation.phase == SourcePhase::Focus
                            {
                                require_idle(&actual)?;
                                open_owned(&mut context, now, &mut effects)?;
                            }
                        } else if context.session.is_some() {
                            if !was_connected
                                || source.started_at_ms <= context.latest_started_at_ms
                            {
                                return Err(error(
                                    "会话切换发生在断连期间或来源身份异常，请核对记录",
                                ));
                            }
                            if context.active_segment_id.is_none() {
                                require_idle(&actual)?;
                            }
                            // A quick skip can hide Rest between polls. Keep only the last
                            // confirmed part of the old record and flag the missing interval.
                            let boundary = actual.active.as_ref().map(|s| {
                                before
                                    .last_observed_at_ms
                                    .max(s.start_at_ms + 1)
                                    .min(now.utc_ms)
                            });
                            if let Some(boundary) = boundary {
                                close_owned(
                                    &mut context,
                                    actual.active.as_ref(),
                                    Instant::new(boundary, now.tz_offset_min),
                                    &mut effects,
                                )?;
                            }
                            context.session = Some(source.clone());
                            context.latest_started_at_ms = source.started_at_ms;
                            context.resume_on_focus = false;
                            // A deliberate manual stop remains stopped until the user starts
                            // the next segment; a genuinely new source session may auto-start.
                            if observation.phase == SourcePhase::Focus {
                                if actual.active.is_none() {
                                    require_idle(&actual)?;
                                }
                                open_owned(&mut context, now, &mut effects)?;
                            }
                            context.gap_detected = true;
                            context.conflict = Some("漏采了两场之间的结束状态：旧段按最后确认时刻暂存，请核对旧段截止时间".into());
                        } else if source.started_at_ms > context.latest_started_at_ms
                            || (!was_initialized
                                && source.started_at_ms == context.latest_started_at_ms)
                        {
                            if !was_connected
                                && was_initialized
                                && !(auto_baseline
                                    && !before.manually_stopped
                                    && actual.active.is_none()
                                    && actual.last_point_kind != Some(PointKind::Pause))
                            {
                                return Err(error("断连期间出现新会话，需要核对后重新建立基线"));
                            }
                            context.latest_started_at_ms = source.started_at_ms;
                            context.session = Some(source.clone());
                            context.manually_stopped = true;
                            context.conflict = None;
                            if !was_initialized {
                                if auto_baseline
                                    && !before.manually_stopped
                                    && actual.active.is_none()
                                    && actual.last_point_kind != Some(PointKind::Pause)
                                    && observation.phase == SourcePhase::Focus
                                {
                                    open_owned(&mut context, now, &mut effects)?;
                                } else {
                                    context.conflict =
                                        Some("连接时专注已经进行中，请手动开始当前分段".into());
                                }
                            } else if actual.active.is_some()
                                || actual.last_point_kind == Some(PointKind::Pause)
                            {
                                context.conflict =
                                    Some("已有无关记录或暂停会话，请先处理，再手动开始分段".into());
                            } else if observation.phase == SourcePhase::Focus {
                                open_owned(&mut context, now, &mut effects)?;
                            }
                            if auto_baseline
                                && !before.manually_stopped
                                && actual.active.is_none()
                                && actual.last_point_kind != Some(PointKind::Pause)
                                && observation.phase == SourcePhase::Paused
                            {
                                context.resume_on_focus = true;
                                context.manually_stopped = false;
                                context.conflict = None;
                            }
                        }
                    } else if context.session.is_some() {
                        if !was_connected && context.active_segment_id.is_some() {
                            return Err(error("断连后专注已结束，结束时间未知，需要人工核对"));
                        }
                        close_owned(&mut context, actual.active.as_ref(), now, &mut effects)?;
                        context.session = None;
                        context.resume_on_focus = false;
                        context.manually_stopped = false;
                        context.conflict = None;
                    }
                    context.connected = true;
                }
                context.source_phase = observation.phase;
            }
            LinkCommand::EndSegment => {
                if context.session.is_none() {
                    return Err(error("没有关联的番茄专注"));
                }
                close_owned(&mut context, actual.active.as_ref(), now, &mut effects)?;
                context.manually_stopped = true;
                context.resume_on_focus = false;
            }
            LinkCommand::StartNext => {
                if !context.connected
                    || context.source_phase != SourcePhase::Focus
                    || now.utc_ms.saturating_sub(context.last_observed_at_ms) > 5_000
                {
                    return Err(error("需要新鲜且正在运行的番茄状态才能开始下一段"));
                }
                if context.session.is_none() || context.active_segment_id.is_some() {
                    return Err(error("当前不能开始下一段"));
                }
                require_idle(&actual)?;
                open_owned(&mut context, now, &mut effects)?;
            }
            LinkCommand::Reconcile { end_at_ms } => {
                if end_at_ms > now.utc_ms {
                    return Err(error("核对截止时间不能晚于当前时间"));
                }
                // Older versions may have left a pointer to a closed/deleted record.
                // Explicit reconciliation may clear that pointer, never an unrelated timer.
                if let Some(id) = context.active_segment_id.clone() {
                    if actual.active.as_ref().map(|s| s.id.as_str()) != Some(id.as_str()) {
                        if let Some(segment) = self.store.find_segment(&id)? {
                            if segment.is_active() {
                                return Err(error("关联记录仍在运行，不能解除身份"));
                            }
                            if !context.pending_notes.contains(&id) {
                                context.pending_notes.push(id);
                            }
                        }
                        context.active_segment_id = None;
                    }
                }
                close_owned(
                    &mut context,
                    actual.active.as_ref(),
                    Instant::new(end_at_ms, now.tz_offset_min),
                    &mut effects,
                )?;
                context.session = None;
                context.initialized = false;
                context.connected = false;
                context.source_phase = SourcePhase::Unknown;
                context.resume_on_focus = false;
                context.manually_stopped = true;
                context.conflict = None;
                context.gap_detected = false;
            }
            LinkCommand::Detach => {
                if context.active_segment_id.is_some() {
                    return Err(error("请先结束或核对当前分段，再停用联动"));
                }
                context.session = None;
                context.initialized = false;
                context.connected = false;
                context.source_phase = SourcePhase::Unknown;
                context.resume_on_focus = false;
                context.conflict = None;
            }
            LinkCommand::SaveNote {
                segment_id,
                revision,
                note,
            } => {
                if note.len() > 64 * 1024 {
                    return Err(error("备注过长"));
                }
                save_note(
                    &self.store,
                    &mut context,
                    &segment_id,
                    revision,
                    Some(note),
                    now,
                    &mut effects,
                )?;
            }
            LinkCommand::SkipNote {
                segment_id,
                revision,
            } => {
                save_note(
                    &self.store,
                    &mut context,
                    &segment_id,
                    revision,
                    None,
                    now,
                    &mut effects,
                )?;
            }
        }
        let serialized = serde_json::to_string(&context).map_err(|_| error("无法保存联动状态"))?;
        let mut comparable = context.clone();
        comparable.last_observed_at_ms = before.last_observed_at_ms;
        if !effects.is_empty() || comparable != before {
            self.store
                .commit_runtime(&effects, RUNTIME_KEY, &serialized, now.utc_ms)?;
        }
        *self.link_live.lock().unwrap_or_else(|e| e.into_inner()) = Some(context.clone());
        Ok(context)
    }
}

fn read_context(store: &impl TimerStore) -> AppResult<LinkState> {
    match store.runtime_state(RUNTIME_KEY)? {
        Some(value) => {
            serde_json::from_str(&value).map_err(|_| error("联动状态损坏，已停止自动处理"))
        }
        None => Ok(LinkState::default()),
    }
}

fn validate_observation(o: &Observation, c: &LinkState, now: i64) -> AppResult<()> {
    if o.observed_at_ms < c.last_observed_at_ms
        || o.observed_at_ms > now.saturating_add(1_000)
        || now.saturating_sub(o.observed_at_ms) > 5_000
    {
        return Err(error("拒绝过期、乱序或未来的番茄状态"));
    }
    if matches!(o.phase, SourcePhase::Focus | SourcePhase::Paused) {
        if !o.count_up {
            return Err(error("本次联动仅支持正计时"));
        }
        let s = o.session.as_ref().ok_or_else(|| error("缺少番茄会话"))?;
        if s.key.is_empty()
            || s.key.len() > 512
            || s.title.trim().is_empty()
            || s.title.len() > 2_000
            || s.started_at_ms <= 0
            || s.started_at_ms > o.observed_at_ms
        {
            return Err(error("番茄会话字段无效"));
        }
    }
    Ok(())
}

fn require_idle(snapshot: &crate::TimerSnapshot) -> AppResult<()> {
    if snapshot.active.is_some() || snapshot.last_point_kind == Some(PointKind::Pause) {
        return Err(error("已有无关记录，不能覆盖或接管"));
    }
    Ok(())
}

fn open_owned(c: &mut LinkState, now: Instant, effects: &mut Vec<Effect>) -> AppResult<()> {
    let source = c.session.as_ref().ok_or_else(|| error("缺少关联专注"))?;
    let mut segment = Segment::new_active(crate::new_id(), now);
    segment.title = source.title.clone();
    segment.is_focus = true;
    segment.validate()?;
    let point = TimePoint::new(crate::new_id(), now, PointKind::Start, PointSource::Plugin)
        .with_segment(segment.id.clone())
        .with_payload(serde_json::json!({"tomatodoSession":source.key,"sourceStartedAt":source.started_at_ms}).to_string());
    c.active_segment_id = Some(segment.id.clone());
    c.manually_stopped = false;
    c.resume_on_focus = false;
    c.conflict = None;
    effects.extend([
        Effect::InsertSegment(segment),
        Effect::InsertTimePoint(point),
    ]);
    Ok(())
}

fn close_owned(
    c: &mut LinkState,
    active: Option<&Segment>,
    now: Instant,
    effects: &mut Vec<Effect>,
) -> AppResult<()> {
    let Some(id) = c.active_segment_id.as_deref() else {
        return Ok(());
    };
    let segment = active
        .filter(|s| s.id == id)
        .ok_or_else(|| error("关联段已被其它操作改变，需要核对"))?;
    let closed = segment.clone().closed(now)?;
    let point = TimePoint::new(crate::new_id(), now, PointKind::Punch, PointSource::Plugin)
        .with_segment(id.to_owned())
        .with_payload(serde_json::json!({"awaitNote":true,"tomatodoSession":c.session.as_ref().map(|s| &s.key)}).to_string());
    if !c.pending_notes.iter().any(|pending| pending == id) {
        c.pending_notes.push(id.to_owned());
    }
    effects.extend([
        Effect::UpdateSegment(closed),
        Effect::InsertTimePoint(point),
    ]);
    c.active_segment_id = None;
    Ok(())
}

fn save_note(
    store: &impl TimerStore,
    c: &mut LinkState,
    id: &str,
    revision: u32,
    note: Option<String>,
    now: Instant,
    effects: &mut Vec<Effect>,
) -> AppResult<()> {
    if !c.pending_notes.iter().any(|pending| pending == id) {
        return Err(error("该段不在待补记队列"));
    }
    let Some(mut segment) = store.find_segment(id)? else {
        if note.is_none() {
            c.pending_notes.retain(|pending| pending != id);
            return Ok(());
        }
        return Err(error("记录已不存在，可确认移出补记队列"));
    };
    if segment.is_active() || segment.revision != revision {
        return Err(error("记录已变化，请刷新后保存"));
    }
    if let Some(note) = note {
        segment.note = note;
        segment.revision += 1;
        effects.push(Effect::UpdateSegment(segment));
    }
    effects.push(Effect::InsertTimePoint(
        TimePoint::new(crate::new_id(), now, PointKind::Edit, PointSource::Plugin)
            .with_segment(id.to_owned())
            .with_payload("{\"tomatodoNoteConfirmed\":true}"),
    ));
    c.pending_notes.retain(|pending| pending != id);
    Ok(())
}
