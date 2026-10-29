//! 启动恢复规则（`docs/domain-model.md` §8）。
//!
//! 检测输入全部由应用层注入：domain 不读时钟、不读设置、不访问数据库。
//! 四类异常共用同一套「检测 → 用户选择 → 产出变更集合」流程。

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::model::{
    Effect, Instant, Phase, PointKind, PointSource, Segment, TimePoint, TimePointId, TimerState,
    Transition,
};

/// 默认长间隔阈值：心跳断档超过 5 分钟视为休眠 / 关机。
pub const DEFAULT_LONG_GAP_MS: i64 = 5 * 60_000;
/// 默认时钟回拨容忍：2 秒内的抖动不算异常。
pub const DEFAULT_CLOCK_TOLERANCE_MS: i64 = 2_000;

/// 恢复提示的原因（ipc-contract §2 `RecoveryInfo.reason`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    /// active 段起始早于今天的日边界。
    CrossDay,
    /// 心跳断档超过阈值（休眠、关机、长时间崩溃）。
    LongGap,
    /// 当前时间早于段起点，或比上次心跳回拨超过容忍值。
    ClockAnomaly,
    /// 上次进程非正常退出，留下了无人接管的 active 段。
    OrphanActive,
}

impl RecoveryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrossDay => "cross_day",
            Self::LongGap => "long_gap",
            Self::ClockAnomaly => "clock_anomaly",
            Self::OrphanActive => "orphan_active",
        }
    }

    /// 该原因下可用的处理方式；`truncate` 仅在存在合法截断点时提供。
    pub fn options(self, cutoff_ms: Option<i64>) -> Vec<RecoveryAction> {
        match self {
            Self::CrossDay => vec![
                RecoveryAction::Truncate,
                RecoveryAction::Extend,
                RecoveryAction::Discard,
            ],
            Self::LongGap if cutoff_ms.is_some() => vec![
                RecoveryAction::Truncate,
                RecoveryAction::Extend,
                RecoveryAction::Discard,
            ],
            Self::LongGap => vec![RecoveryAction::Extend, RecoveryAction::Discard],
            Self::ClockAnomaly => vec![RecoveryAction::Extend, RecoveryAction::Keep],
            Self::OrphanActive => vec![
                RecoveryAction::Extend,
                RecoveryAction::Discard,
                RecoveryAction::Keep,
            ],
        }
    }
}

/// 用户对恢复提示的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// 截断到边界（跨日 = 日边界；长间隔 = 最后心跳；时钟异常 = now）。
    Truncate,
    /// 计入到当前时刻并结束。
    Extend,
    /// 丢弃这段记录（软删除）。
    Discard,
    /// 保持原状，稍后手动处理。
    Keep,
}

impl RecoveryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Truncate => "truncate",
            Self::Extend => "extend",
            Self::Discard => "discard",
            Self::Keep => "keep",
        }
    }

    pub fn allows(self, reason: RecoveryReason, cutoff_ms: Option<i64>) -> bool {
        reason.options(cutoff_ms).contains(&self)
    }
}

/// 检测上下文；`now_ms` 与日边界都由应用层按本地时区算好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryContext {
    pub now_ms: i64,
    /// 今天日窗口起点（本地日边界，UTC 毫秒）。
    pub day_window_start_ms: i64,
    /// 上一次进程写入的最后心跳；从未写入为 `None`。
    pub last_heartbeat_ms: Option<i64>,
    /// 上一次进程是否正常退出。
    pub clean_shutdown: bool,
    pub long_gap_ms: i64,
    pub clock_tolerance_ms: i64,
}

impl Default for RecoveryContext {
    fn default() -> Self {
        Self {
            now_ms: 0,
            day_window_start_ms: 0,
            last_heartbeat_ms: None,
            clean_shutdown: true,
            long_gap_ms: DEFAULT_LONG_GAP_MS,
            clock_tolerance_ms: DEFAULT_CLOCK_TOLERANCE_MS,
        }
    }
}

/// 检测结果。`required=false` 时仍可能带 `reason`（例如无 active 段的时钟回拨只提示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryCheck {
    pub required: bool,
    pub reason: Option<RecoveryReason>,
    /// `truncate` 的落点。
    pub cutoff_ms: Option<i64>,
}

impl RecoveryCheck {
    pub fn none() -> Self {
        Self {
            required: false,
            reason: None,
            cutoff_ms: None,
        }
    }

    pub fn options(&self) -> Vec<RecoveryAction> {
        self.reason
            .map(|reason| reason.options(self.cutoff_ms))
            .unwrap_or_default()
    }
}

/// 检测优先级：时钟异常 → 跨日 → 长间隔 → 孤儿 active。
pub fn detect(active: Option<&Segment>, ctx: &RecoveryContext) -> RecoveryCheck {
    let clock_backwards = ctx
        .last_heartbeat_ms
        .is_some_and(|heartbeat| ctx.now_ms + ctx.clock_tolerance_ms < heartbeat);

    let Some(active) = active else {
        // 没有进行中的记录时，时钟回拨只提示、不阻塞。
        return RecoveryCheck {
            required: false,
            reason: clock_backwards.then_some(RecoveryReason::ClockAnomaly),
            cutoff_ms: None,
        };
    };

    if ctx.now_ms < active.start_at_ms || clock_backwards {
        return RecoveryCheck {
            required: true,
            reason: Some(RecoveryReason::ClockAnomaly),
            cutoff_ms: Some(ctx.now_ms.max(active.start_at_ms)),
        };
    }

    if active.start_at_ms < ctx.day_window_start_ms {
        return RecoveryCheck {
            required: true,
            reason: Some(RecoveryReason::CrossDay),
            cutoff_ms: Some(ctx.day_window_start_ms),
        };
    }

    let gap_ms = ctx
        .last_heartbeat_ms
        .map_or(ctx.now_ms - active.start_at_ms, |heartbeat| {
            ctx.now_ms - heartbeat
        });
    if gap_ms > ctx.long_gap_ms {
        // 从未写过心跳时没有可信的截断点，只能计入或丢弃。
        let cutoff = ctx
            .last_heartbeat_ms
            .filter(|heartbeat| *heartbeat > active.start_at_ms);
        return RecoveryCheck {
            required: true,
            reason: Some(RecoveryReason::LongGap),
            cutoff_ms: cutoff,
        };
    }

    if !ctx.clean_shutdown {
        return RecoveryCheck {
            required: true,
            reason: Some(RecoveryReason::OrphanActive),
            cutoff_ms: None,
        };
    }

    RecoveryCheck::none()
}

/// 一次恢复决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryResolution {
    pub reason: RecoveryReason,
    pub action: RecoveryAction,
    pub at: Instant,
    pub cutoff_ms: Option<i64>,
    pub point_id: TimePointId,
}

/// 把恢复决策转换为事务内的变更集合。
///
/// - `Extend` / `Truncate`：关闭 active 段并写 `finish` 事实（`source=system`）。
/// - `Discard`：软删除 active 段并写 `delete` 事实。
/// - `Keep`：不改数据，返回当前状态。
pub fn resolve(
    state: &TimerState,
    resolution: &RecoveryResolution,
) -> DomainResult<Transition> {
    if !resolution
        .action
        .allows(resolution.reason, resolution.cutoff_ms)
    {
        return Err(DomainError::InvalidRange);
    }

    let Some(active) = state.active.as_ref() else {
        return Ok(Transition {
            state: state.clone(),
            effects: Vec::new(),
            pending_note_segment_id: None,
            undo: None,
        });
    };

    let payload = format!(
        "{{\"reason\":\"{}\",\"action\":\"{}\"}}",
        resolution.reason.as_str(),
        resolution.action.as_str()
    );

    let (end_ms, kind) = match resolution.action {
        RecoveryAction::Keep => {
            return Ok(Transition {
                state: state.clone(),
                effects: Vec::new(),
                pending_note_segment_id: None,
                undo: None,
            })
        }
        RecoveryAction::Extend => (resolution.at.utc_ms, PointKind::Finish),
        RecoveryAction::Truncate => (
            resolution.cutoff_ms.ok_or(DomainError::InvalidRange)?,
            PointKind::Finish,
        ),
        RecoveryAction::Discard => (resolution.at.utc_ms, PointKind::Delete),
    };

    if end_ms <= active.start_at_ms {
        return Err(DomainError::InvalidRange);
    }
    if end_ms > resolution.at.utc_ms {
        return Err(DomainError::InvalidRange);
    }

    let point_at = Instant::new(end_ms, active.tz_offset_min);
    let point = TimePoint::new(
        resolution.point_id.clone(),
        resolution.at,
        kind,
        PointSource::System,
    )
    .with_segment(active.id.clone())
    .with_payload(payload);

    let effects = if resolution.action == RecoveryAction::Discard {
        vec![
            Effect::SoftDeleteSegment {
                id: active.id.clone(),
                at_ms: end_ms,
            },
            Effect::InsertTimePoint(point),
        ]
    } else {
        let closed = active.clone().closed(point_at)?;
        closed.validate()?;
        vec![
            Effect::UpdateSegment(closed),
            Effect::InsertTimePoint(point),
        ]
    };

    Ok(Transition {
        state: TimerState {
            phase: Phase::Idle,
            active: None,
            last_point_kind: Some(kind),
        },
        effects,
        pending_note_segment_id: None,
        undo: None,
    })
}
