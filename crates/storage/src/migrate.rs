use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};

/// 有序迁移表。每个迁移的 SQL 在编译期嵌入，运行时用 sha256 校验。
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "seed_categories",
        sql: include_str!("../migrations/0002_seed_categories.sql"),
    },
    Migration {
        version: 3,
        name: "record_sessions",
        sql: include_str!("../migrations/0003_record_sessions.sql"),
    },
];

/// 当前程序支持的最高 schema 版本。
pub const LATEST_VERSION: i64 = 3;

const MIGRATION_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS migration (
  version       INTEGER PRIMARY KEY,
  name          TEXT    NOT NULL,
  checksum      TEXT    NOT NULL,
  applied_at_ms INTEGER NOT NULL
) STRICT;
";

/// 应用尚未执行的迁移；已执行的迁移会校验校验和（防止手改历史迁移）。
pub fn apply(conn: &mut Connection, now_ms: i64) -> StorageResult<()> {
    conn.execute_batch(MIGRATION_TABLE_SQL)?;

    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version > LATEST_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: user_version,
            supported: LATEST_VERSION,
        });
    }

    for migration in MIGRATIONS {
        let checksum = hex_sha256(migration.sql);
        let applied: Option<String> = conn
            .query_row(
                "SELECT checksum FROM migration WHERE version = ?1",
                params![migration.version],
                |row| row.get(0),
            )
            .optional()?;

        match applied {
            Some(stored) if stored == checksum => continue,
            Some(_) => {
                return Err(StorageError::ChecksumMismatch {
                    version: migration.version,
                })
            }
            None => {}
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO migration (version, name, checksum, applied_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![migration.version, migration.name, checksum, now_ms],
        )?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
    }

    Ok(())
}

fn hex_sha256(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
