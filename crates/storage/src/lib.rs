//! DotLog 持久层：SQLite 单写连接、迁移与仓储实现。
//!
//! 边界（见 `docs/architecture.md` §3.1）：不依赖 UI、Tauri 或插件；只依赖
//! `dotlog-domain` 与 `rusqlite`。

mod db;
mod error;
pub mod migrate;
mod port_impl;
pub mod repo;

pub use db::Database;
pub use error::{StorageError, StorageResult};
/// 生成 UUIDv7（时间有序）；与持久层共用同一实现。
pub use dotlog_application::new_id;
