use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::{StorageError, StorageResult};
use crate::migrate;
use crate::repo;

/// 单个写连接（`docs/architecture.md` §5）：所有写操作串行，避免 `SQLITE_BUSY`。
/// `Clone` 只共享同一连接，供计时服务与插件宿主共用（M5）。
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// 打开（或创建）数据库文件并执行迁移。
    pub fn open(path: impl AsRef<Path>, now_ms: i64) -> StorageResult<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_connection(Connection::open(path)?, now_ms)
    }

    /// 内存库，供测试使用。
    pub fn open_in_memory(now_ms: i64) -> StorageResult<Self> {
        Self::from_connection(Connection::open_in_memory()?, now_ms)
    }

    fn from_connection(mut conn: Connection, now_ms: i64) -> StorageResult<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )?;
        migrate::apply(&mut conn, now_ms)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 在一个 `BEGIN IMMEDIATE` 事务中执行；回调返回错误即整体回滚。
    pub fn with_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let mut guard = self.lock();
        let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// 只读查询（WAL 下不阻塞写入）。
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> StorageResult<T>) -> StorageResult<T> {
        f(&self.lock())
    }

    /// 在线快照到指定文件（`VACUUM INTO`），并校验产物可读。
    pub fn backup_to(&self, path: &Path) -> StorageResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `VACUUM INTO` 要求目标文件不存在。
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        if let Err(error) = self.read(|conn| repo::vacuum_into(conn, path)) {
            let _ = std::fs::remove_file(path);
            return Err(match error {
                StorageError::Db(inner) => StorageError::BackupFailed(inner.to_string()),
                other => other,
            });
        }

        let snapshot = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let ok = repo::quick_check_ok(&snapshot);
        drop(snapshot);
        if !ok {
            let _ = std::fs::remove_file(path);
            return Err(StorageError::BackupFailed("备份文件完整性校验失败".to_string()));
        }
        Ok(())
    }

    /// 用备份文件覆盖当前库并重跑迁移；调用方需先做安全副本。
    pub fn restore_from(&self, path: &Path, now_ms: i64) -> StorageResult<()> {
        if !path.exists() {
            return Err(StorageError::RestoreInvalid("备份文件不存在".to_string()));
        }
        let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if !repo::quick_check_ok(&source) {
            return Err(StorageError::RestoreInvalid(
                "备份文件未通过完整性校验".to_string(),
            ));
        }
        let version: i64 = source.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > migrate::LATEST_VERSION {
            return Err(StorageError::RestoreInvalid(
                "备份来自更新版本的应用".to_string(),
            ));
        }

        {
            let mut guard = self.lock();
            {
                let backup = rusqlite::backup::Backup::new(&source, &mut guard)?;
                backup.run_to_completion(64, Duration::from_millis(10), None)?;
            }
            // 备份文件是 rollback-journal 模式，恢复后切回 WAL。
            guard.query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0))?;
            migrate::apply(&mut guard, now_ms)?;
            if !repo::quick_check_ok(&guard) {
                return Err(StorageError::RestoreInvalid(
                    "恢复后完整性校验失败".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // 单写连接：持有者 panic 后仍可继续服务，避免整个应用不可用。
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
