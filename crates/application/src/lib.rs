//! DotLog 应用层：用例编排、事务边界与端口定义。
//!
//! 依赖规则（`docs/architecture.md` §3.1）：只依赖 `dotlog-domain`；
//! 具体数据库、Tauri、插件实现均在外部。

pub mod backup;
pub mod dto;
mod error;
pub mod export;
pub mod import;
pub mod ports;
pub mod recovery;
mod service;
pub mod settings;
pub mod stats;
mod timefmt;

pub use dto::{
    CategorySecondsView, CategoryView, DiagnosticsView, ExportPayload, ImportPreviewView,
    ImportResultView, ImportRowView, IntegrityView, RecoveryInfoView, StatsBucketView, StatsView,
    TableCountView,
};
pub use error::{AppError, AppResult};
pub use export::ExportFormat;
pub use ports::{
    link_state, outbox_state, CategoryRow, Clock, IntegrationLinkRow, IntegrityReport, OutboxRecord,
    PointRecord, TimerSnapshot, TimerStore,
};
pub use service::TimerService;
pub use dotlog_domain::Granularity;

/// 生成 UUIDv7（时间有序）。持久层负责分配，领域层只消费。
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
