//! DotLog 领域层：计时状态机、事实模型与不变量。
//!
//! 设计约束（见 `docs/domain-model.md`）：不依赖数据库、网络、UI、OS API 与
//! async runtime；时间一律以 UTC epoch 毫秒传入，本地时区偏移随段一起保存。

mod edit;
mod error;
mod model;
pub mod recovery;
mod timer;
pub mod day;
pub mod stats;

pub use day::{
    clip, day_window, gaps, local_date, parse_day_window, totals, Clip, DayPolicy, DayTotals,
    DEFAULT_DAY_START_MINUTES, DEFAULT_FOCUS_MIN_MINUTES,
};
pub use edit::{backfill, delete, merge, resize, split, BackfillParams};
pub use error::{DomainError, DomainResult};
pub use model::{
    EditUndo, Effect, Instant, NewIds, Phase, PointKind, PointSource, PunchMode, Segment,
    SegmentId, TimePoint, TimePointId, TimerState, Transition, UndoToken, DEFAULT_UNDO_WINDOW_MS,
};
pub use recovery::{
    detect as detect_recovery, resolve as resolve_recovery, RecoveryAction, RecoveryCheck,
    RecoveryContext, RecoveryReason, RecoveryResolution, DEFAULT_CLOCK_TOLERANCE_MS,
    DEFAULT_LONG_GAP_MS,
};
pub use stats::{
    bucket as stats_bucket, bucket_windows, BucketStats, CategorySeconds, Granularity,
};
pub use timer::{finish, pause, punch, resume, start, undo};
