use thiserror::Error;

use crate::model::Segment;

pub type DomainResult<T> = Result<T, DomainError>;

/// 领域错误。`code()` 直接对应 `docs/ipc-contract.md` 中的错误码，
/// 前端按 code 分支处理，不应依赖展示文案。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    /// T7：空闲状态下执行 punch / pause / finish。
    #[error("当前没有进行中的记录")]
    NotRecording,

    /// T8：已存在 active 段时再次 start / resume。
    #[error("已有进行中的记录，请先结束或恢复")]
    ActiveExists,

    /// 结束时间不晚于开始时间。
    #[error("时间区间不合法：结束必须晚于开始")]
    InvalidRange,

    /// 编辑/补录与已有段重叠，携带冲突段列表（domain-model §8）。
    #[error("与已有记录重叠")]
    Overlap(Vec<Segment>),

    /// 补录跨度超过 24 小时（domain-model §9）。
    #[error("补录跨度不能超过 24 小时")]
    BackfillTooLong,

    /// 起点或终点落在未来。
    #[error("不能记录未来时间")]
    FutureTime,

    /// 给 active 段设置终点（应使用打点或结束命令）。
    #[error("进行中的记录请用打点或结束来收尾")]
    ActiveHasNoEnd,

    /// 合并的段数不足两段。
    #[error("合并至少需要两段记录")]
    MergeNeedsTwo,

    /// 合并包含进行中的段。
    #[error("进行中的记录不能合并")]
    MergeActive,

    /// 撤销窗口已过。
    #[error("撤销窗口已过，请到时间线直接修改")]
    UndoExpired,

    /// 撤销目标不可用（例如链式打点后的下一段已填写内容）。
    #[error("这一步无法撤销：后续记录已有内容")]
    UndoUnavailable,

    /// 段结构自身不满足不变量（I2 / I4）。
    #[error("时间段数据不合法")]
    InvalidSegment,
}

impl DomainError {
    /// 稳定的错误码，供 IPC 层原样返回给前端。
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotRecording => "E_NOT_RECORDING",
            Self::ActiveExists => "E_ACTIVE_EXISTS",
            Self::InvalidRange => "E_INVALID_RANGE",
            Self::Overlap(_) => "E_OVERLAP",
            Self::BackfillTooLong => "E_INVALID_RANGE",
            Self::FutureTime => "E_INVALID_RANGE",
            Self::ActiveHasNoEnd => "E_INVALID_RANGE",
            Self::MergeNeedsTwo => "E_INVALID_RANGE",
            Self::MergeActive => "E_INVALID_RANGE",
            Self::UndoExpired => "E_UNDO_EXPIRED",
            Self::UndoUnavailable => "E_UNDO_UNAVAILABLE",
            Self::InvalidSegment => "E_INVALID_SEGMENT",
        }
    }
}
