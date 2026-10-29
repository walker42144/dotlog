//! 存储层集成测试：迁移、唯一 active 约束、事务回滚与段的读写往返。

use dotlog_domain::{Instant, PointKind, PointSource, Segment, TimePoint};
use dotlog_storage::{new_id, repo, Database, StorageError};

const NOW: i64 = 1_700_000_000_000;
const TZ: i32 = 480;

fn at(ms: i64) -> Instant {
    Instant::new(ms, TZ)
}

fn new_segment(start_ms: i64) -> Segment {
    Segment::new_active(new_id(), at(start_ms))
}

fn new_point(at_ms: i64, kind: PointKind, segment_id: &str) -> TimePoint {
    TimePoint::new(new_id(), at(at_ms), kind, PointSource::Button).with_segment(segment_id)
}

#[test]
fn migrations_apply_once_and_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dotlog.db");

    let first = Database::open(&path, NOW).unwrap();
    drop(first);

    // 第二次打开只做校验，不重复执行。
    let second = Database::open(&path, NOW).unwrap();
    let (version, count): (i64, i64) = second
        .read(|conn| {
            let version = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let count = conn.query_row("SELECT COUNT(*) FROM migration", [], |row| row.get(0))?;
            Ok((version, count))
        })
        .unwrap();

    assert_eq!(version, 3);
    assert_eq!(count, 3);
}

#[test]
fn database_creates_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("dotlog.db");
    Database::open(&path, NOW).unwrap();
    assert!(path.exists());
}

#[test]
fn single_active_segment_is_enforced_by_index() {
    let db = Database::open_in_memory(NOW).unwrap();
    let first = new_segment(NOW);
    let second = new_segment(NOW + 60_000);

    let result = db.with_transaction(|tx| {
        repo::insert_segment(tx, &first, NOW)?;
        repo::insert_segment(tx, &second, NOW + 60_000)?;
        Ok(())
    });

    assert!(result.is_err(), "第二条 active 段必须被唯一索引拒绝");
    let active = db.read(repo::find_active_segment).unwrap();
    assert!(active.is_none(), "失败事务必须整体回滚");
}

#[test]
fn transaction_rolls_back_on_error() {
    let db = Database::open_in_memory(NOW).unwrap();
    let segment = new_segment(NOW);

    let result: Result<(), StorageError> = db.with_transaction(|tx| {
        repo::insert_segment(tx, &segment, NOW)?;
        Err(StorageError::Io(std::io::Error::other("模拟业务失败")))
    });
    assert!(result.is_err());

    let count: i64 = db
        .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM time_segment", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn segment_roundtrip_and_last_point_kind() {
    let db = Database::open_in_memory(NOW).unwrap();
    let mut segment = new_segment(NOW);
    segment.title = "写代码".to_string();

    db.with_transaction(|tx| {
        repo::insert_segment(tx, &segment, NOW)?;
        repo::insert_time_point(tx, &new_point(NOW, PointKind::Start, &segment.id))?;
        Ok(())
    })
    .unwrap();

    let loaded = db
        .read(repo::find_active_segment)
        .unwrap()
        .expect("应能读回 active 段");
    assert_eq!(loaded, segment);
    assert_eq!(
        db.read(repo::last_point_kind).unwrap(),
        Some(PointKind::Start)
    );

    // 打点：关闭段 + 写入 punch 事实。
    let closed = segment.clone().closed(at(NOW + 25 * 60_000)).unwrap();
    db.with_transaction(|tx| {
        repo::update_segment(tx, &closed, NOW + 25 * 60_000)?;
        repo::insert_time_point(
            tx,
            &new_point(NOW + 25 * 60_000, PointKind::Punch, &closed.id),
        )?;
        Ok(())
    })
    .unwrap();

    assert!(db.read(repo::find_active_segment).unwrap().is_none());
    assert_eq!(
        db.read(repo::last_point_kind).unwrap(),
        Some(PointKind::Punch)
    );

    let stored_end: Option<i64> = db
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT end_at_ms FROM time_segment WHERE id = ?1",
                [&segment.id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(stored_end, Some(NOW + 25 * 60_000));
}

#[test]
fn soft_delete_active_segment_satisfies_check_constraint() {
    let db = Database::open_in_memory(NOW).unwrap();
    let segment = new_segment(NOW);

    db.with_transaction(|tx| {
        repo::insert_segment(tx, &segment, NOW)?;
        repo::soft_delete_segment(tx, &segment.id, NOW + 1_000)?;
        Ok(())
    })
    .expect("active 段软删除必须同时补齐 end_at_ms");

    let (status, end): (String, Option<i64>) = db
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT status, end_at_ms FROM time_segment WHERE id = ?1",
                [&segment.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(status, "deleted");
    assert_eq!(end, Some(NOW + 1_000));
    assert!(db.read(repo::find_active_segment).unwrap().is_none());
}

#[test]
fn apply_effects_writes_expected_rows() {
    let db = Database::open_in_memory(NOW).unwrap();
    let segment = new_segment(NOW);
    let point = new_point(NOW, PointKind::Start, &segment.id);

    db.with_transaction(|tx| {
        repo::apply_effects(
            tx,
            &[
                dotlog_domain::Effect::InsertSegment(segment.clone()),
                dotlog_domain::Effect::InsertTimePoint(point.clone()),
            ],
            NOW,
        )
    })
    .unwrap();

    let points: i64 = db
        .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM time_point", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(points, 1);
    assert_eq!(
        db.read(repo::last_point_kind).unwrap(),
        Some(PointKind::Start)
    );
}

#[test]
fn mark_point_undone_hides_latest_point() {
    let db = Database::open_in_memory(NOW).unwrap();
    let segment = new_segment(NOW);
    let start_point = new_point(NOW, PointKind::Start, &segment.id);
    let undo_point = new_point(NOW + 1_000, PointKind::Undo, &segment.id);

    db.with_transaction(|tx| {
        repo::apply_effects(
            tx,
            &[
                dotlog_domain::Effect::InsertSegment(segment.clone()),
                dotlog_domain::Effect::InsertTimePoint(start_point),
                dotlog_domain::Effect::InsertTimePoint(undo_point.clone()),
            ],
            NOW + 1_000,
        )?;
        repo::apply_effects(
            tx,
            &[dotlog_domain::Effect::MarkPointUndone {
                id: undo_point.id,
                at_ms: NOW + 2_000,
            }],
            NOW + 2_000,
        )
    })
    .unwrap();

    let latest = db
        .read(repo::latest_point)
        .unwrap()
        .expect("撤销事实后应回退到上一条事实");
    assert_eq!(latest.kind, PointKind::Start);
    assert_eq!(
        db.read(repo::last_point_kind).unwrap(),
        Some(PointKind::Start)
    );
}

#[test]
fn runtime_state_roundtrip_and_upsert() {
    let db = Database::open_in_memory(NOW).unwrap();

    assert_eq!(
        db.read(|conn| repo::runtime_get(conn, "heartbeat.wall_ms"))
            .unwrap(),
        None
    );

    db.read(|conn| repo::runtime_set(conn, "heartbeat.wall_ms", "100", NOW))
        .unwrap();
    db.read(|conn| repo::runtime_set(conn, "heartbeat.wall_ms", "200", NOW + 1_000))
        .unwrap();

    let (value, updated): (String, i64) = db
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT value, updated_at_ms FROM runtime_state WHERE key = 'heartbeat.wall_ms'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(value, "200");
    assert_eq!(updated, NOW + 1_000);
    assert_eq!(db.read(repo::schema_version).unwrap(), 3);
}

#[test]
fn integrity_report_detects_overlap_and_counts_active() {
    let db = Database::open_in_memory(NOW).unwrap();

    // 两段区间重叠的已关闭段（结束晚于下一段开始）。
    let first = new_segment(NOW).closed(at(NOW + 10 * 60_000)).unwrap();
    let second = new_segment(NOW + 5 * 60_000)
        .closed(at(NOW + 20 * 60_000))
        .unwrap();

    db.with_transaction(|tx| {
        repo::insert_segment(tx, &first, NOW)?;
        repo::insert_segment(tx, &second, NOW)?;
        Ok(())
    })
    .unwrap();

    let report = db
        .read(|conn| repo::integrity_report(conn, NOW + 30 * 60_000))
        .unwrap();
    assert!(report.quick_check_ok);
    assert_eq!(report.active_count, 0);
    assert_eq!(report.overlap_pairs, 1);
    assert_eq!(report.longest_open_seconds, 0);
    assert!(!report.is_healthy(), "存在重叠应判为不健康");

    // 追加一条 active 段后，活动计数与超长未关闭时长都应被检出。
    let active = new_segment(NOW - 2 * 3_600_000);
    db.with_transaction(|tx| repo::insert_segment(tx, &active, NOW))
        .unwrap();

    let report = db.read(|conn| repo::integrity_report(conn, NOW)).unwrap();
    assert_eq!(report.active_count, 1);
    assert_eq!(report.longest_open_seconds, 2 * 3_600);
}

#[test]
fn table_counts_lists_every_counted_table() {
    let db = Database::open_in_memory(NOW).unwrap();
    let counts = db.read(repo::table_counts).unwrap();

    let names: Vec<&str> = counts.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, repo::COUNTED_TABLES);

    let category = counts
        .iter()
        .find(|(name, _)| name == "category")
        .map(|(_, count)| *count)
        .unwrap();
    assert!(category >= 8, "迁移 0002 应已种子分类，实际 {category}");
}

#[test]
fn ten_thousand_segments_stay_within_budget() {
    use std::time::Instant as StdInstant;

    const TOTAL: i64 = 10_000;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dotlog.db");
    let db = Database::open(&path, NOW).unwrap();
    let insert_started = StdInstant::now();
    db.with_transaction(|tx| {
        for index in 0..TOTAL {
            let start = NOW + index * 60_000;
            let segment = new_segment(start).closed(at(start + 60_000)).unwrap();
            repo::insert_segment(tx, &segment, start)?;
        }
        Ok(())
    })
    .unwrap();
    let insert_elapsed = insert_started.elapsed();

    // 单日查询：10 000 段约 7 天，这里取中间一天。
    let day_from = NOW + 3 * 86_400_000;
    let query_started = StdInstant::now();
    let day = db
        .read(|conn| repo::segments_overlapping(conn, day_from, day_from + 86_400_000))
        .unwrap();
    let query_elapsed = query_started.elapsed();

    let integrity_started = StdInstant::now();
    let report = db
        .read(|conn| repo::integrity_report(conn, NOW + TOTAL * 60_000))
        .unwrap();
    let integrity_elapsed = integrity_started.elapsed();

    // 冷启动代理：文件库重新打开（含迁移校验），覆盖启动阶段的主要开销。
    drop(db);
    let reopen_started = StdInstant::now();
    let reopened = Database::open(&path, NOW).unwrap();
    let reopen_elapsed = reopen_started.elapsed();

    assert!(!day.is_empty(), "单日查询不应为空");
    assert!(report.quick_check_ok);
    assert_eq!(report.overlap_pairs, 0);
    assert_eq!(
        reopened.read(repo::schema_version).unwrap(),
        3,
        "重新打开后 schema 版本应保持"
    );

    println!(
        "10k 段：写入 {:?}，单日查询 {:?}（{} 段），完整性自检 {:?}，冷启动 {:?}",
        insert_elapsed,
        query_elapsed,
        day.len(),
        integrity_elapsed,
        reopen_elapsed
    );

    // 预算按 debug 构建宽松设定；release 下远低于此值。
    assert!(
        insert_elapsed.as_millis() < 10_000,
        "10k 段写入超预算：{insert_elapsed:?}"
    );
    assert!(
        query_elapsed.as_millis() < 2_000,
        "单日查询超预算：{query_elapsed:?}"
    );
    assert!(
        integrity_elapsed.as_millis() < 2_000,
        "完整性自检超预算：{integrity_elapsed:?}"
    );
    assert!(
        reopen_elapsed.as_millis() < 1_500,
        "冷启动超预算：{reopen_elapsed:?}"
    );
}

#[test]
fn new_id_is_time_ordered() {
    let a = new_id();
    let b = new_id();
    assert_eq!(a.len(), 36);
    assert!(a < b, "UUIDv7 应可按时间排序：{a} < {b}");
}

// ---------- outbox 与 integration_link（M5）----------

#[test]
fn outbox_enqueue_dedupes_by_key_and_takes_due_in_order() {
    let db = Database::open_in_memory(NOW).unwrap();

    assert!(db
        .read(|conn| repo::outbox_enqueue(
            conn,
            Some("memos:a:publish"),
            "memos.publish",
            "{}",
            NOW
        ))
        .unwrap());
    // 重复去重键被忽略。
    assert!(!db
        .read(|conn| repo::outbox_enqueue(
            conn,
            Some("memos:a:publish"),
            "memos.publish",
            "{}",
            NOW
        ))
        .unwrap());
    // 无去重键允许重复。
    assert!(db
        .read(|conn| repo::outbox_enqueue(conn, None, "memos.publish", "{}", NOW + 1))
        .unwrap());

    // 到期按创建顺序取走并转 inflight（新事件无 next_retry 视为立即到期）。
    let due = db
        .read(|conn| repo::outbox_take_due(conn, NOW + 5, 10))
        .unwrap();
    assert_eq!(due.len(), 2);
    assert!(due.iter().all(|record| record.state == "inflight"));
    assert!(due.iter().all(|record| record.attempts == 0));

    // 设置未来重试时间后，未到期不可取。
    let first = due[0].id.clone();
    db.read(|conn| repo::outbox_mark_retry(conn, &first, "网络不可达", NOW + 60_000, NOW + 6))
        .unwrap();
    assert!(db
        .read(|conn| repo::outbox_take_due(conn, NOW + 7, 10))
        .unwrap()
        .is_empty());

    // 5 分钟后：first 的退避时间已到（重新入队），另一条的 inflight 租约也已过期（回收）。
    let recovered = db
        .read(|conn| repo::outbox_take_due(conn, NOW + 5 * 60_000 + 10, 10))
        .unwrap();
    assert_eq!(recovered.len(), 2);
    assert!(recovered.iter().any(|record| record.id == first));
    assert!(recovered
        .iter()
        .any(|record| record.id != first && record.attempts == 0));
}

#[test]
fn outbox_retry_backoff_ends_in_failed_and_manual_retry_resets() {
    let db = Database::open_in_memory(NOW).unwrap();
    db.read(|conn| repo::outbox_enqueue(conn, Some("memos:b:publish"), "memos.publish", "{}", NOW))
        .unwrap();
    let id = db
        .read(|conn| {
            Ok(repo::outbox_list(conn, Some("pending"), 10).unwrap()[0]
                .id
                .clone())
        })
        .unwrap();

    // 退避：30s → 2min → 10min → 1h → 6h；第 6 次失败转 failed。
    let gaps = [30_000i64, 120_000, 600_000, 3_600_000, 21_600_000];
    let mut now = NOW;
    for (attempt, gap) in gaps.iter().enumerate() {
        now += 1;
        db.read(|conn| repo::outbox_mark_retry(conn, &id, "网络不可达", now + gap, now))
            .unwrap();
        let record = db
            .read(|conn| Ok(repo::outbox_list(conn, None, 10).unwrap().remove(0)))
            .unwrap();
        assert_eq!(record.attempts, attempt as i64 + 1);
        assert_eq!(
            record.state,
            "pending",
            "第 {} 次重试后仍应为 pending",
            attempt + 1
        );
    }

    now += 1;
    db.read(|conn| repo::outbox_mark_retry(conn, &id, "网络不可达", now, now))
        .unwrap();
    let record = db
        .read(|conn| {
            Ok(repo::outbox_list(conn, Some("failed"), 10)
                .unwrap()
                .remove(0))
        })
        .unwrap();
    assert_eq!(record.state, "failed");
    assert_eq!(record.attempts, 6);
    assert_eq!(record.last_error.as_deref(), Some("网络不可达"));

    // 手动重试：attempts 清零、立即到期；done 的事件不允许复活。
    assert!(db
        .read(|conn| repo::outbox_retry(conn, &id, NOW + 10))
        .unwrap());
    let record = db
        .read(|conn| {
            Ok(repo::outbox_list(conn, Some("pending"), 10)
                .unwrap()
                .remove(0))
        })
        .unwrap();
    assert_eq!(record.attempts, 0);
    assert_eq!(record.next_retry_at_ms, Some(NOW + 10));

    db.read(|conn| repo::outbox_mark_done(conn, &id, NOW + 11))
        .unwrap();
    assert!(!db
        .read(|conn| repo::outbox_retry(conn, &id, NOW + 12))
        .unwrap());

    // 概览计数。
    let counts = db.read(repo::outbox_counts).unwrap();
    assert_eq!(counts, vec![("done".to_string(), 1)]);
}

#[test]
fn integration_link_upsert_and_last_sync() {
    let db = Database::open_in_memory(NOW).unwrap();
    // integration_link.segment_id 有外键约束，先落一条真实段。
    let segment = new_segment(NOW);
    db.read(|conn| repo::insert_segment(conn, &segment, NOW))
        .unwrap();
    let segment_id = segment.id.clone();
    let other_id;
    {
        let other = new_segment(NOW + 60_000);
        // active 段唯一，第二条必须是已关闭段。
        let mut closed = other;
        closed.end_at_ms = Some(NOW + 120_000);
        other_id = closed.id.clone();
        db.read(|conn| repo::insert_segment(conn, &closed, NOW))
            .unwrap();
    }

    assert!(db
        .read(|conn| repo::integration_link_get(conn, &segment_id, "memos"))
        .unwrap()
        .is_none());

    db.read(|conn| {
        repo::integration_link_upsert(conn, &segment_id, "memos", None, "pending", None, NOW)
    })
    .unwrap();
    // 发布成功：写 external_id。
    db.read(|conn| {
        repo::integration_link_upsert(
            conn,
            &segment_id,
            "memos",
            Some("memo-42"),
            "published",
            None,
            NOW + 1_000,
        )
    })
    .unwrap();

    let link = db
        .read(|conn| repo::integration_link_get(conn, &segment_id, "memos"))
        .unwrap()
        .unwrap();
    assert_eq!(link.external_id.as_deref(), Some("memo-42"));
    assert_eq!(link.state, "published");
    assert_eq!(link.last_synced_at_ms, Some(NOW + 1_000));

    // 失败记录不产生成功同步时间。
    db.read(|conn| {
        repo::integration_link_upsert(
            conn,
            &other_id,
            "memos",
            None,
            "failed",
            Some("连接超时"),
            NOW + 2_000,
        )
    })
    .unwrap();
    assert_eq!(
        db.read(|conn| repo::integration_last_sync(conn, "memos"))
            .unwrap(),
        Some(NOW + 1_000)
    );
    assert_eq!(
        db.read(|conn| repo::integration_last_sync(conn, "ai"))
            .unwrap(),
        None
    );
}
