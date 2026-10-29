use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};

/// UUIDv7 字符串（可排序），由持久层生成后传入领域层。
pub type SegmentId = String;
pub type TimePointId = String;

/// 撤销窗口默认 15 秒（`punch.undo_window_ms` 的设置默认值）。
pub const DEFAULT_UNDO_WINDOW_MS: i64 = 15_000;

/// 一个时间点（UTC 毫秒 + 记录时的本地时区偏移）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instant {
    pub utc_ms: i64,
    pub tz_offset_min: i32,
}

impl Instant {
    pub fn new(utc_ms: i64, tz_offset_min: i32) -> Self {
        Self {
            utc_ms,
            tz_offset_min,
        }
    }
}

/// 持久化相位：只有两相。暂停不是独立状态（见 domain-model §2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Recording,
}

/// 时间点种类。`time_point` 只追加，撤销通过 `undone_at_ms` 标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointKind {
    Start,
    Resume,
    Punch,
    Pause,
    Finish,
    Undo,
    Delete,
    Edit,
    Backfill,
}

/// 事实来源，用于统计与排查（区分快捷键 / 托盘 / 二次启动）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointSource {
    Button,
    Hotkey,
    Tray,
    SecondInstance,
    Api,
    /// 系统自动写入（恢复处理、启动核对）。
    System,
    /// 插件（集成）写入的审计事实。
    Plugin,
}

/// 打点模式（ADR-006）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PunchMode {
    /// 打点后关闭当前段回到 Idle（默认）。
    Stop,
    /// 打点后立即开启下一段。
    Chain,
}

/// 新建实体所需的 id；由持久层分配后传入，领域层不依赖序列器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIds {
    pub segment_id: SegmentId,
    pub point_id: TimePointId,
}

impl NewIds {
    pub fn new(segment_id: impl Into<SegmentId>, point_id: impl Into<TimePointId>) -> Self {
        Self {
            segment_id: segment_id.into(),
            point_id: point_id.into(),
        }
    }
}

/// 时间段：用户看到与编辑的记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub start_at_ms: i64,
    pub end_at_ms: Option<i64>,
    pub title: String,
    pub note: String,
    pub tags: Vec<String>,
    pub category_id: Option<String>,
    pub is_focus: bool,
    pub is_ignored: bool,
    pub tz_offset_min: i32,
    pub revision: u32,
}

impl Segment {
    /// 新建一个 active 段（`end_at_ms = None`，`revision = 1`）。
    pub fn new_active(id: SegmentId, at: Instant) -> Self {
        Self {
            id,
            start_at_ms: at.utc_ms,
            end_at_ms: None,
            title: String::new(),
            note: String::new(),
            tags: Vec::new(),
            category_id: None,
            is_focus: false,
            is_ignored: false,
            tz_offset_min: at.tz_offset_min,
            revision: 1,
        }
    }

    pub fn is_active(&self) -> bool {
        self.end_at_ms.is_none()
    }

    /// 已结束段的时长；active 段按 `now` 计算。
    pub fn duration_ms(&self, now_ms: i64) -> i64 {
        self.end_at_ms.unwrap_or(now_ms) - self.start_at_ms
    }

    /// 关闭当前段，返回关闭后的新版本（revision + 1）。
    pub fn closed(mut self, at: Instant) -> DomainResult<Self> {
        if at.utc_ms <= self.start_at_ms {
            return Err(DomainError::InvalidRange);
        }
        self.end_at_ms = Some(at.utc_ms);
        self.revision += 1;
        debug_assert!(self.end_at_ms.is_some());
        Ok(self)
    }

    /// 重新打开已关闭段（撤销用）。
    pub fn reopened(mut self) -> Self {
        self.end_at_ms = None;
        self.revision += 1;
        self
    }

    /// 不变量校验：I2（关闭段 end > start）、I4（active 段 end 为 None）。
    pub fn validate(&self) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::InvalidSegment);
        }
        match self.end_at_ms {
            None => Ok(()),
            Some(end) if end > self.start_at_ms => Ok(()),
            Some(_) => Err(DomainError::InvalidRange),
        }
    }

    /// 两段是否重叠（半开区间 `[start, end)`；active 段用 `now` 作为临时终点）。
    pub fn overlaps(&self, other: &Segment, now_ms: i64) -> bool {
        let a_end = self.end_at_ms.unwrap_or(now_ms);
        let b_end = other.end_at_ms.unwrap_or(now_ms);
        self.start_at_ms < b_end && other.start_at_ms < a_end
    }
}

/// 时间点：一次事实，只追加。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimePoint {
    pub id: TimePointId,
    pub occurred_at_ms: i64,
    pub tz_offset_min: i32,
    pub kind: PointKind,
    pub source: PointSource,
    pub segment_id: Option<SegmentId>,
    pub undone_at_ms: Option<i64>,
    /// 撤销/编辑的审计载荷，例如 `undo_of=42`。
    pub payload: Option<String>,
}

impl TimePoint {
    pub fn new(id: TimePointId, at: Instant, kind: PointKind, source: PointSource) -> Self {
        Self {
            id,
            occurred_at_ms: at.utc_ms,
            tz_offset_min: at.tz_offset_min,
            kind,
            source,
            segment_id: None,
            undone_at_ms: None,
            payload: None,
        }
    }

    pub fn with_segment(mut self, segment_id: impl Into<SegmentId>) -> Self {
        self.segment_id = Some(segment_id.into());
        self
    }

    pub fn with_payload(mut self, payload: impl Into<String>) -> Self {
        self.payload = Some(payload.into());
        self
    }
}

/// 状态机可见的当前状态。持久层负责把它映射到数据库。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimerState {
    pub phase: Phase,
    pub active: Option<Segment>,
    /// 最近一条时间点的种类，用于派生「暂停中」。
    pub last_point_kind: Option<PointKind>,
}

impl TimerState {
    pub fn idle() -> Self {
        Self::default()
    }

    pub fn recording(active: Segment) -> Self {
        Self {
            phase: Phase::Recording,
            active: Some(active),
            last_point_kind: Some(PointKind::Start),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.phase == Phase::Recording
    }

    /// 暂停中 = Idle 且最近事实是 pause（domain-model §2）。
    pub fn is_paused(&self) -> bool {
        self.phase == Phase::Idle && self.last_point_kind == Some(PointKind::Pause)
    }

    /// 未开始 = Idle 且当天没有任何段；M0 先以「无历史事实」近似。
    pub fn is_fresh(&self) -> bool {
        self.phase == Phase::Idle && self.last_point_kind.is_none()
    }
}

/// 一次命令需要落库的变更集合；由 application 在单个事务内执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    InsertSegment(Segment),
    UpdateSegment(Segment),
    SoftDeleteSegment { id: SegmentId, at_ms: i64 },
    /// 撤销编辑类操作时恢复段（含从软删除恢复，`deleted_at_ms` 清空）。
    RestoreSegment(Segment),
    InsertTimePoint(TimePoint),
    /// 撤销时标记被撤销的事实（`time_point.undone_at_ms`），事实本身不删除。
    MarkPointUndone { id: TimePointId, at_ms: i64 },
}

/// 编辑类操作的撤销数据：恢复编辑前的段快照，软删除本次新建的段。
///
/// 序列化为 `time_point.payload`（JSON），重启后仍能重建撤销凭据。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EditUndo {
    pub restore: Vec<Segment>,
    pub remove_ids: Vec<SegmentId>,
}

impl EditUndo {
    pub fn to_payload(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_payload(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

/// 撤销凭据：由上一次命令返回，15 秒内有效。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoToken {
    pub point_id: TimePointId,
    pub kind: PointKind,
    pub issued_at_ms: i64,
    pub window_ms: i64,
    /// 被本次命令关闭的段（撤销时重开）。
    pub closed_segment: Option<Segment>,
    /// 链式打点新建的下一段（撤销时软删除）。
    pub opened_segment_id: Option<SegmentId>,
    /// 编辑类操作（edit / backfill / delete / split / merge）的撤销数据。
    pub edit: Option<EditUndo>,
}

impl UndoToken {
    pub fn expired(&self, now_ms: i64) -> bool {
        now_ms - self.issued_at_ms > self.window_ms
    }
}

/// 命令执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub state: TimerState,
    pub effects: Vec<Effect>,
    /// 打点后需要补记的段 id（UI 浮出补记框）。
    pub pending_note_segment_id: Option<SegmentId>,
    pub undo: Option<UndoToken>,
}
