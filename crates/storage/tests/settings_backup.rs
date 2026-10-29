//! 分类种子、设置项读写与备份/恢复（`docs/data-dictionary.md` §7 / §8）。

use dotlog_application::TimerStore;
use dotlog_domain::{Instant, Segment};
use dotlog_storage::{new_id, repo, Database};

const NOW: i64 = 1_700_000_000_000;
const TZ: i32 = 480;

fn at(ms: i64) -> Instant {
    Instant::new(ms, TZ)
}

fn active_segment(start_ms: i64) -> Segment {
    Segment::new_active(new_id(), at(start_ms))
}

#[test]
fn migration_seeds_eight_categories() {
    let db = Database::open_in_memory(NOW).unwrap();
    let categories = db.categories().unwrap();

    assert_eq!(categories.len(), 8);
    let ids: Vec<&str> = categories.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "cat-study",
            "cat-work",
            "cat-life",
            "cat-sport",
            "cat-reading",
            "cat-fun",
            "cat-sleep",
            "cat-other",
        ]
    );
    assert!(categories.iter().all(|row| row.color.starts_with('#')));
    assert!(categories.iter().all(|row| row.parent_id.is_empty()));
}

#[test]
fn settings_upsert_overrides_and_resets() {
    let db = Database::open_in_memory(NOW).unwrap();

    db.settings_upsert(
        "appearance",
        &[
            ("theme".to_string(), "\"dark\"".to_string()),
            ("font_scale".to_string(), "1.0".to_string()),
        ],
        NOW,
    )
    .unwrap();
    assert_eq!(db.settings("appearance").unwrap().len(), 2);

    // 同名 key 覆盖而非追加。
    db.settings_upsert(
        "appearance",
        &[("theme".to_string(), "\"light\"".to_string())],
        NOW + 1_000,
    )
    .unwrap();
    let rows = db.settings("appearance").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .find(|(key, _)| key == "theme")
            .map(|(_, value)| value.as_str()),
        Some("\"light\"")
    );

    // 单 key 重置。
    db.settings_reset("appearance", Some("theme")).unwrap();
    assert_eq!(db.settings("appearance").unwrap().len(), 1);

    // 整个命名空间重置。
    db.settings_reset("appearance", None).unwrap();
    assert!(db.settings("appearance").unwrap().is_empty());
}

#[test]
fn settings_are_isolated_by_scope() {
    let db = Database::open_in_memory(NOW).unwrap();
    db.settings_upsert(
        "recording",
        &[("day_start_minutes".to_string(), "240".to_string())],
        NOW,
    )
    .unwrap();

    assert_eq!(db.settings("recording").unwrap().len(), 1);
    assert!(db.settings("appearance").unwrap().is_empty());
}

#[test]
fn backup_and_restore_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("dotlog.db");
    let backup_path = dir.path().join("backups").join("dotlog-1.db");
    let db = Database::open(&db_path, NOW).unwrap();

    let kept = active_segment(NOW);
    db.with_transaction(|tx| repo::insert_segment(tx, &kept, NOW))
        .unwrap();
    db.backup_to(&backup_path).unwrap();
    assert!(backup_path.exists(), "备份文件应写入磁盘");

    // 备份后改动当前库：删除原段、新增另一段。
    let extra = active_segment(NOW + 3_600_000);
    db.with_transaction(|tx| {
        repo::soft_delete_segment(tx, &kept.id, NOW + 60_000)?;
        repo::insert_segment(tx, &extra, NOW + 60_000)
    })
    .unwrap();

    db.restore_from(&backup_path, NOW).unwrap();

    let segments = db
        .read(|conn| repo::segments_overlapping(conn, 0, i64::MAX / 2))
        .unwrap();
    assert_eq!(segments.len(), 1, "恢复后应只剩备份里的段");
    assert_eq!(segments[0].id, kept.id);

    // 恢复后连接仍可用（迁移已重跑、WAL 模式已切回）。
    let closed = active_segment(NOW + 7_200_000)
        .closed(at(NOW + 8_200_000))
        .unwrap();
    db.with_transaction(|tx| repo::insert_segment(tx, &closed, NOW))
        .unwrap();
    let version: i64 = db
        .read(|conn| Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?))
        .unwrap();
    assert_eq!(version, 3);
}

#[test]
fn restore_rejects_missing_or_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("dotlog.db"), NOW).unwrap();

    let missing = db
        .restore_from(&dir.path().join("nope.db"), NOW)
        .unwrap_err();
    assert_eq!(missing.code(), "E_RESTORE_INVALID");

    // 不是 SQLite 数据库 → quick_check 失败。
    let bad_path = dir.path().join("bad.db");
    std::fs::write(&bad_path, b"not a database").unwrap();
    let invalid = db.restore_from(&bad_path, NOW).unwrap_err();
    assert_eq!(invalid.code(), "E_RESTORE_INVALID");
}
