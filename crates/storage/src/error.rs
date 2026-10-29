use thiserror::Error;

pub type StorageResult<T> = Result<T, StorageError>;

/// 持久层错误。业务层会把它映射为 IPC 错误码（见 `docs/ipc-contract.md` §5）。
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("数据库错误：{0}")]
    Db(#[from] rusqlite::Error),

    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),

    /// 已应用的迁移文件被改动；拒绝启动以免数据错乱。
    #[error("迁移 {version} 的校验和不一致，数据库可能被手动修改")]
    ChecksumMismatch { version: i64 },

    /// 数据库 schema 比当前程序新（用户降级了版本）。
    #[error("数据库版本 {found} 高于当前程序支持的 {supported}")]
    SchemaTooNew { found: i64, supported: i64 },

    /// 在线快照失败（磁盘满、目标不可写等）。
    #[error("备份失败：{0}")]
    BackupFailed(String),

    /// 备份文件不存在、损坏或版本过高，拒绝覆盖当前库。
    #[error("备份文件不可用：{0}")]
    RestoreInvalid(String),
}

impl StorageError {
    /// 与 `docs/ipc-contract.md` §5 对齐的稳定错误码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::ChecksumMismatch { .. } | Self::SchemaTooNew { .. } => "E_DB_CORRUPT",
            Self::BackupFailed(_) => "E_BACKUP_FAILED",
            Self::RestoreInvalid(_) => "E_RESTORE_INVALID",
            Self::Db(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::DatabaseBusy
                    || e.code == rusqlite::ErrorCode::DatabaseLocked =>
            {
                "E_DB_BUSY"
            }
            _ => "E_DB",
        }
    }
}
