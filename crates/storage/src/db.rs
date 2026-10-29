use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{OptionalExtension, Connection, OpenFlags, Transaction, TransactionBehavior};

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
            return Err(StorageError::BackupFailed(
                "目标备份已存在，请使用新的文件名".into(),
            ));
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
            return Err(StorageError::BackupFailed(
                "备份文件完整性校验失败".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate and migrate a private candidate before touching the live connection.
    /// Keep a rollback snapshot under the same connection lock throughout replacement.
    pub fn restore_from(&self, path: &Path, now_ms: i64) -> StorageResult<()> {
        let invalid = |e: String| StorageError::RestoreInvalid(e);
        if !path.is_file() {
            return Err(invalid("备份文件不存在".into()));
        }
        let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| invalid(e.to_string()))?;
        if !repo::quick_check_ok(&source) {
            return Err(invalid("备份完整性校验失败".into()));
        }
        let mut candidate = Connection::open_in_memory()?;
        copy_database(&source, &mut candidate).map_err(|e| invalid(e.to_string()))?;
        let validate = || -> StorageResult<()> {
            let version: i64 = candidate.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            if !(1..=migrate::LATEST_VERSION).contains(&version) {
                return Err(invalid("不是受支持的 DotLog 备份".into()));
            }
            for table in [
                "migration",
                "category",
                "tag",
                "time_segment",
                "time_point",
                "segment_tag",
                "integration_link",
                "outbox_event",
                "setting",
                "runtime_state",
            ] {
                let count: i64 = candidate.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )?;
                if count != 1 {
                    return Err(invalid(format!("缺少 DotLog 数据表：{table}")));
                }
            }
            if version >= 3 {
                candidate.prepare("SELECT segment_id,session_id FROM session_segment LIMIT 0")?;
                candidate
                    .prepare("SELECT id,status,anchor_segment_id FROM record_session LIMIT 0")?;
            }
            let applied: i64 = candidate.query_row(
                "SELECT count(*) FROM migration WHERE version BETWEEN 1 AND ?1",
                [version],
                |r| r.get(0),
            )?;
            if applied != version {
                return Err(invalid("迁移记录不完整".into()));
            }
            Ok(())
        };
        validate().map_err(|e| invalid(e.to_string()))?;
        migrate::apply(&mut candidate, now_ms).map_err(|e| invalid(e.to_string()))?;
        candidate.execute_batch("PRAGMA foreign_keys=ON;")?;
        let integrity =
            repo::integrity_report(&candidate, now_ms).map_err(|e| invalid(e.to_string()))?;
        let foreign_errors: i64 =
            candidate.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if !integrity.is_healthy() || foreign_errors != 0 {
            return Err(invalid("备份含重叠记录或关联损坏".into()));
        }
        repo::table_counts(&candidate).map_err(|e| invalid(e.to_string()))?;
        // Restored queue entries must never resume outbound traffic without user action.
        candidate.execute(
            "UPDATE setting SET value='false' WHERE scope LIKE 'plugin:%' AND key='enabled'",
            [],
        )?;
        let local: Option<String> = candidate.query_row(
            "SELECT value FROM setting WHERE scope='local-plugins' AND key='registry'", [], |r| r.get(0)
        ).optional()?;
        if let Some(raw) = local {
            let mut registry: serde_json::Value = serde_json::from_str(&raw).map_err(|_| invalid("本地插件注册表损坏".into()))?;
            let entries = registry.as_object_mut().ok_or_else(|| invalid("本地插件注册表格式错误".into()))?;
            for entry in entries.values_mut() { if !entry.is_object() { return Err(invalid("本地插件记录格式错误".into())); } entry["enabled"] = serde_json::json!(false); }
            candidate.execute("UPDATE setting SET value=?1 WHERE scope='local-plugins' AND key='registry'", [registry.to_string()])?;
        }
        let mut guard = self.lock();
        let mut rollback = Connection::open_in_memory()?;
        copy_database(&guard, &mut rollback)?;
        if let Err(error) = copy_database(&candidate, &mut guard) {
            copy_database(&rollback, &mut guard)?;
            return Err(invalid(format!("恢复失败，已回退：{error}")));
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // 单写连接：持有者 panic 后仍可继续服务，避免整个应用不可用。
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn copy_database(source: &Connection, target: &mut Connection) -> StorageResult<()> {
    let backup = rusqlite::backup::Backup::new(source, target)?;
    backup.run_to_completion(64, Duration::from_millis(10), None)?;
    Ok(())
}
