use dotlog_application::{
    CategoryRow, IntegrationLinkRow, IntegrityReport, OutboxRecord,
};
use dotlog_domain::{Effect, PointKind, PointSource, Segment, TimePoint};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::{StorageError, StorageResult};

const SEGMENT_COLUMNS: &str = "id, start_at_ms, end_at_ms, start_tz_offset_min, \
     title, note, category_id, is_focus, is_ignored, revision";

/// 当前唯一的 active 段（由 `ux_segment_single_active` 保证至多一条）。
pub fn find_active_segment(conn: &Connection) -> StorageResult<Option<Segment>> {
    let sql = format!("SELECT {SEGMENT_COLUMNS} FROM time_segment WHERE status = 'active' LIMIT 1");
    Ok(conn.query_row(&sql, [], map_segment).optional()?)
}

/// 与 `[from_ms, to_ms)` 相交的非删除段，按开始时间升序。
pub fn segments_overlapping(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> StorageResult<Vec<Segment>> {
    let sql = format!(
        "SELECT {SEGMENT_COLUMNS} FROM time_segment
         WHERE status <> 'deleted' AND start_at_ms < ?2 AND (end_at_ms IS NULL OR end_at_ms > ?1)
         ORDER BY start_at_ms ASC"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![from_ms, to_ms], map_segment)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 按 id 读取未删除的段。
pub fn find_segment_by_id(conn: &Connection, id: &str) -> StorageResult<Option<Segment>> {
    let sql =
        format!("SELECT {SEGMENT_COLUMNS} FROM time_segment WHERE id = ?1 AND status <> 'deleted'");
    Ok(conn.query_row(&sql, params![id], map_segment).optional()?)
}

/// 最近一条未被撤销的事实。
pub struct PointRow {
    pub id: String,
    pub kind: PointKind,
    pub occurred_at_ms: i64,
    pub segment_id: Option<String>,
    /// 编辑类事实的撤销载荷（JSON），其它类型为 None。
    pub payload: Option<String>,
}

pub fn latest_point(conn: &Connection) -> StorageResult<Option<PointRow>> {
    let sql = "SELECT id, kind, occurred_at_ms, segment_id, payload FROM time_point
               WHERE undone_at_ms IS NULL
               ORDER BY occurred_at_ms DESC, created_at_ms DESC
               LIMIT 1";
    let raw = conn
        .query_row(sql, [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .optional()?;

    raw.map(|(id, kind, occurred_at_ms, segment_id, payload)| {
        let kind = point_kind_from_str(&kind)
            .ok_or_else(|| StorageError::Db(rusqlite::Error::InvalidQuery))?;
        Ok(PointRow {
            id,
            kind,
            occurred_at_ms,
            segment_id,
            payload,
        })
    })
    .transpose()
}

/// 标记事实为已撤销（事实本身保留）。
pub fn mark_point_undone(conn: &Connection, id: &str, at_ms: i64) -> StorageResult<()> {
    conn.execute(
        "UPDATE time_point SET undone_at_ms = ?2 WHERE id = ?1 AND undone_at_ms IS NULL",
        params![id, at_ms],
    )?;
    Ok(())
}

/// 最近一条未被撤销的事实种类，用于派生「暂停中」状态。
pub fn last_point_kind(conn: &Connection) -> StorageResult<Option<PointKind>> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM time_point
             WHERE undone_at_ms IS NULL
             ORDER BY occurred_at_ms DESC, created_at_ms DESC
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(kind.as_deref().and_then(point_kind_from_str))
}

pub fn insert_segment(conn: &Connection, segment: &Segment, now_ms: i64) -> StorageResult<()> {
    let status = if segment.end_at_ms.is_some() {
        "closed"
    } else {
        "active"
    };
    // 结束时区偏移暂与开始时一致（中国无夏令时）；跨时区编辑在 M2 处理。
    let end_tz = segment.end_at_ms.map(|_| segment.tz_offset_min);
    conn.execute(
        "INSERT INTO time_segment (
            id, start_at_ms, end_at_ms, start_tz_offset_min, end_tz_offset_min,
            title, note, category_id, is_focus, is_ignored,
            status, revision, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            segment.id,
            segment.start_at_ms,
            segment.end_at_ms,
            segment.tz_offset_min,
            end_tz,
            segment.title,
            note_value(&segment.note),
            segment.category_id,
            i64::from(segment.is_focus),
            i64::from(segment.is_ignored),
            status,
            segment.revision as i64,
            now_ms,
            now_ms,
        ],
    )?;
    Ok(())
}

pub fn update_segment(conn: &Connection, segment: &Segment, now_ms: i64) -> StorageResult<()> {
    let status = if segment.end_at_ms.is_some() {
        "closed"
    } else {
        "active"
    };
    let end_tz = segment.end_at_ms.map(|_| segment.tz_offset_min);
    let changed = conn.execute(
        "UPDATE time_segment SET
            start_at_ms = ?2, end_at_ms = ?3, start_tz_offset_min = ?4, end_tz_offset_min = ?5,
            title = ?6, note = ?7, category_id = ?8, is_focus = ?9, is_ignored = ?10,
            status = ?11, revision = ?12, updated_at_ms = ?13
         WHERE id = ?1",
        params![
            segment.id,
            segment.start_at_ms,
            segment.end_at_ms,
            segment.tz_offset_min,
            end_tz,
            segment.title,
            note_value(&segment.note),
            segment.category_id,
            i64::from(segment.is_focus),
            i64::from(segment.is_ignored),
            status,
            segment.revision as i64,
            now_ms,
        ],
    )?;
    if changed == 0 {
        return Err(StorageError::Db(rusqlite::Error::QueryReturnedNoRows));
    }
    Ok(())
}

/// 软删除。CHECK 约束要求非 active 的段必须有 `end_at_ms`，因此 active 段会顺手关闭。
pub fn soft_delete_segment(conn: &Connection, id: &str, at_ms: i64) -> StorageResult<()> {
    conn.execute(
        "UPDATE time_segment SET
            status = 'deleted',
            end_at_ms = COALESCE(end_at_ms, ?2),
            end_tz_offset_min = COALESCE(end_tz_offset_min, start_tz_offset_min),
            deleted_at_ms = ?2,
            updated_at_ms = ?2,
            revision = revision + 1
         WHERE id = ?1 AND status IN ('active', 'closed')",
        params![id, at_ms],
    )?;
    Ok(())
}

/// 撤销编辑时恢复段：覆盖全部字段并清除软删除标记（`deleted_at_ms` 置空）。
pub fn restore_segment(conn: &Connection, segment: &Segment, now_ms: i64) -> StorageResult<()> {
    let status = if segment.end_at_ms.is_some() {
        "closed"
    } else {
        "active"
    };
    let end_tz = segment.end_at_ms.map(|_| segment.tz_offset_min);
    let changed = conn.execute(
        "UPDATE time_segment SET
            start_at_ms = ?2, end_at_ms = ?3, start_tz_offset_min = ?4, end_tz_offset_min = ?5,
            title = ?6, note = ?7, category_id = ?8, is_focus = ?9, is_ignored = ?10,
            status = ?11, revision = ?12, deleted_at_ms = NULL, updated_at_ms = ?13
         WHERE id = ?1",
        params![
            segment.id,
            segment.start_at_ms,
            segment.end_at_ms,
            segment.tz_offset_min,
            end_tz,
            segment.title,
            note_value(&segment.note),
            segment.category_id,
            i64::from(segment.is_focus),
            i64::from(segment.is_ignored),
            status,
            segment.revision as i64,
            now_ms,
        ],
    )?;
    if changed == 0 {
        return Err(StorageError::Db(rusqlite::Error::QueryReturnedNoRows));
    }
    Ok(())
}

/// 历史页查询：与 `[from_ms, to_ms)` 相交的非删除段，按开始时间倒序，支持标题/备注模糊搜索。
/// 返回 `(当前页, 总数)`。
pub fn history_segments(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    text: Option<&str>,
    limit: i64,
    offset: i64,
) -> StorageResult<(Vec<Segment>, i64)> {
    let where_clause = "status <> 'deleted' AND start_at_ms < ?2 AND (end_at_ms IS NULL OR end_at_ms > ?1)
        AND (?3 IS NULL OR title LIKE '%' || ?3 || '%' OR note LIKE '%' || ?3 || '%')";
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM time_segment WHERE {where_clause}"),
        params![from_ms, to_ms, text],
        |row| row.get(0),
    )?;
    let sql = format!(
        "SELECT {SEGMENT_COLUMNS} FROM time_segment WHERE {where_clause}
         ORDER BY start_at_ms DESC LIMIT ?4 OFFSET ?5"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![from_ms, to_ms, text, limit, offset], map_segment)?;
    let items = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((items, total))
}

pub fn insert_time_point(conn: &Connection, point: &TimePoint) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO time_point (
            id, occurred_at_ms, tz_offset_min, kind, source, segment_id, payload, undone_at_ms, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?2)",
        params![
            point.id,
            point.occurred_at_ms,
            point.tz_offset_min,
            point_kind_str(point.kind),
            point_source_str(point.source),
            point.segment_id,
            point.payload,
            point.undone_at_ms,
        ],
    )?;
    Ok(())
}

/// 在同一个事务内按顺序应用领域层产出的变更集合。
pub fn apply_effects(
    conn: &Connection,
    effects: &[Effect],
    now_ms: i64,
) -> StorageResult<()> {
    for effect in effects {
        match effect {
            Effect::InsertSegment(segment) => insert_segment(conn, segment, now_ms)?,
            Effect::UpdateSegment(segment) => update_segment(conn, segment, now_ms)?,
            Effect::SoftDeleteSegment { id, at_ms } => soft_delete_segment(conn, id, *at_ms)?,
            Effect::RestoreSegment(segment) => restore_segment(conn, segment, now_ms)?,
            Effect::InsertTimePoint(point) => insert_time_point(conn, point)?,
            Effect::MarkPointUndone { id, at_ms } => mark_point_undone(conn, id, *at_ms)?,
        }
    }
    Ok(())
}

fn map_segment(row: &Row<'_>) -> rusqlite::Result<Segment> {
    Ok(Segment {
        id: row.get(0)?,
        start_at_ms: row.get(1)?,
        end_at_ms: row.get(2)?,
        tz_offset_min: row.get(3)?,
        title: row.get(4)?,
        note: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        category_id: row.get(6)?,
        is_focus: row.get::<_, i64>(7)? != 0,
        is_ignored: row.get::<_, i64>(8)? != 0,
        // 标签关联在 M4 前进度暂不加载（statistics 不依赖标签）。
        tags: Vec::new(),
        revision: row.get::<_, i64>(9)? as u32,
    })
}

/// 未归档分类，按 `sort_order` 排序。
pub fn list_categories(conn: &Connection) -> StorageResult<Vec<CategoryRow>> {
    let mut statement = conn.prepare(
        "SELECT id, name, color, parent_id FROM category
         WHERE archived_at_ms IS NULL ORDER BY sort_order, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(CategoryRow {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
            parent_id: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn find_category(conn: &Connection, id: &str) -> StorageResult<Option<CategoryRow>> {
    Ok(conn
        .query_row(
            "SELECT id, name, color, parent_id FROM category WHERE id = ?1",
            params![id],
            |row| {
                Ok(CategoryRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    parent_id: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// 某命名空间的全部设置项：`(key, JSON 值)`，按 key 排序。
pub fn setting_rows(conn: &Connection, scope: &str) -> StorageResult<Vec<(String, String)>> {
    let mut statement =
        conn.prepare("SELECT key, value FROM setting WHERE scope = ?1 ORDER BY key")?;
    let rows = statement.query_map(params![scope], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn upsert_setting(
    conn: &Connection,
    scope: &str,
    key: &str,
    value: &str,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO setting (scope, key, value, updated_at_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        params![scope, key, value, now_ms],
    )?;
    Ok(())
}

/// `key = None` 时清空整个命名空间。
pub fn delete_setting(conn: &Connection, scope: &str, key: Option<&str>) -> StorageResult<()> {
    match key {
        Some(key) => {
            conn.execute(
                "DELETE FROM setting WHERE scope = ?1 AND key = ?2",
                params![scope, key],
            )?;
        }
        None => {
            conn.execute("DELETE FROM setting WHERE scope = ?1", params![scope])?;
        }
    }
    Ok(())
}

/// 在线一致快照（`docs/data-dictionary.md` §8）。
pub fn vacuum_into(conn: &Connection, path: &std::path::Path) -> StorageResult<()> {
    conn.execute("VACUUM INTO ?1", params![path.to_string_lossy()])?;
    Ok(())
}

/// `PRAGMA quick_check` 是否通过。
pub fn quick_check_ok(conn: &Connection) -> bool {
    conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map(|value| value == "ok")
        .unwrap_or(false)
}

/// 启动完整性自检（`docs/data-dictionary.md` §9）。
///
/// 重叠按 `start_at_ms` 升序扫描：若下一条的起点早于上一条的终点，即为一处重叠。
/// active 段以 `now` 作为临时终点参与比较（正常数据中 active 不应与任何段重叠）。
pub fn integrity_report(conn: &Connection, now_ms: i64) -> StorageResult<IntegrityReport> {
    let quick_check_ok = quick_check_ok(conn);
    let active_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM time_segment WHERE status = 'active'",
        [],
        |row| row.get(0),
    )?;

    let mut statement = conn.prepare(
        "SELECT start_at_ms, end_at_ms FROM time_segment
         WHERE status IN ('active','closed') ORDER BY start_at_ms ASC",
    )?;
    let mut rows = statement.query([])?;
    let mut previous_end: Option<i64> = None;
    let mut overlap_pairs = 0_i64;
    let mut longest_open_seconds = 0_i64;

    while let Some(row) = rows.next()? {
        let start: i64 = row.get(0)?;
        let end: Option<i64> = row.get(1)?;
        if previous_end.is_some_and(|previous| previous > start) {
            overlap_pairs += 1;
        }
        let effective_end = end.unwrap_or(now_ms);
        if end.is_none() {
            let seconds = (now_ms - start).max(0) / 1_000;
            longest_open_seconds = longest_open_seconds.max(seconds);
        }
        previous_end = Some(match previous_end {
            Some(previous) => previous.max(effective_end),
            None => effective_end,
        });
    }

    Ok(IntegrityReport {
        quick_check_ok,
        active_count,
        overlap_pairs,
        longest_open_seconds,
    })
}

/// 诊断包中统计的表（顺序固定，便于逐项比对）。
pub const COUNTED_TABLES: &[&str] = &[
    "category",
    "tag",
    "time_segment",
    "time_point",
    "segment_tag",
    "outbox_event",
    "setting",
];

/// 各表行数；表名来自固定常量，不拼接外部输入。
pub fn table_counts(conn: &Connection) -> StorageResult<Vec<(String, i64)>> {
    let mut result = Vec::with_capacity(COUNTED_TABLES.len());
    for table in COUNTED_TABLES {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))?;
        result.push(((*table).to_string(), count));
    }
    Ok(result)
}

/// 运行时状态读取（心跳、退出标记等）。
pub fn runtime_get(conn: &Connection, key: &str) -> StorageResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM runtime_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

/// 运行时状态 upsert。
pub fn runtime_set(conn: &Connection, key: &str, value: &str, now_ms: i64) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO runtime_state (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        params![key, value, now_ms],
    )?;
    Ok(())
}

/// 当前 schema 版本。
pub fn schema_version(conn: &Connection) -> StorageResult<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

// ---------- outbox（M5 · `docs/plugin-sdk.md` §6）----------

/// 入队；`dedupe_key` 唯一，重复入队忽略（`ON CONFLICT DO NOTHING`）。
pub fn outbox_enqueue(
    conn: &Connection,
    dedupe_key: Option<&str>,
    event_type: &str,
    payload: &str,
    now_ms: i64,
) -> StorageResult<bool> {
    let id = dotlog_application::new_id();
    let inserted = conn.execute(
        "INSERT INTO outbox_event (id, dedupe_key, type, payload, state, attempts, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?5)
         ON CONFLICT(dedupe_key) DO NOTHING",
        params![id, dedupe_key, event_type, payload, now_ms],
    )?;
    Ok(inserted > 0)
}

/// 取出到期事件并置为 `inflight`。到期定义：`pending` 且到达重试时间，
/// 或 `inflight` 的 5 分钟租约已过期（崩溃回收）。
pub fn outbox_take_due(
    conn: &Connection,
    now_ms: i64,
    limit: u32,
) -> StorageResult<Vec<OutboxRecord>> {
    let lease_until = now_ms + 5 * 60_000;
    let sql = "UPDATE outbox_event SET state = 'inflight', next_retry_at_ms = ?1, updated_at_ms = ?2
               WHERE id IN (
                 SELECT id FROM outbox_event
                 WHERE (state = 'pending' AND (next_retry_at_ms IS NULL OR next_retry_at_ms <= ?2))
                    OR (state = 'inflight' AND next_retry_at_ms <= ?2)
                 ORDER BY created_at_ms ASC
                 LIMIT ?3
               )
               RETURNING id, dedupe_key, type, payload, state, attempts, next_retry_at_ms,
                         last_error, created_at_ms, updated_at_ms";
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![lease_until, now_ms, limit], map_outbox)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn outbox_mark_done(conn: &Connection, id: &str, now_ms: i64) -> StorageResult<()> {
    conn.execute(
        "UPDATE outbox_event SET state = 'done', last_error = NULL, next_retry_at_ms = NULL,
           updated_at_ms = ?2 WHERE id = ?1",
        params![id, now_ms],
    )?;
    Ok(())
}

/// 失败退避：尝试次数 +1，达到上限（6 次）后转 `failed`。
pub fn outbox_mark_retry(
    conn: &Connection,
    id: &str,
    error: &str,
    next_retry_at_ms: i64,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "UPDATE outbox_event SET attempts = attempts + 1,
           state = CASE WHEN attempts + 1 >= 6 THEN 'failed' ELSE 'pending' END,
           last_error = ?2, next_retry_at_ms = ?3, updated_at_ms = ?4
         WHERE id = ?1",
        params![id, error, next_retry_at_ms, now_ms],
    )?;
    Ok(())
}

/// 不可恢复失败：直接 `failed`，等待手动重试。
pub fn outbox_mark_failed(conn: &Connection, id: &str, error: &str, now_ms: i64) -> StorageResult<()> {
    conn.execute(
        "UPDATE outbox_event SET state = 'failed', last_error = ?2, next_retry_at_ms = NULL,
           updated_at_ms = ?3 WHERE id = ?1",
        params![id, error, now_ms],
    )?;
    Ok(())
}

/// 集成页队列查询；按创建时间倒序。
pub fn outbox_list(
    conn: &Connection,
    state: Option<&str>,
    limit: u32,
) -> StorageResult<Vec<OutboxRecord>> {
    let base = "SELECT id, dedupe_key, type, payload, state, attempts, next_retry_at_ms,
                       last_error, created_at_ms, updated_at_ms
                FROM outbox_event";
    let (sql, has_filter) = match state {
        Some(_) => (format!("{base} WHERE state = ?1 ORDER BY created_at_ms DESC LIMIT ?2"), true),
        None => (format!("{base} ORDER BY created_at_ms DESC LIMIT ?1"), false),
    };
    let mut statement = conn.prepare(&sql)?;
    let map_rows = |row: &Row<'_>| map_outbox(row);
    let rows = if has_filter {
        statement.query_map(params![state, limit], map_rows)?
    } else {
        statement.query_map(params![limit], map_rows)?
    };
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 手动重试：回到 `pending`、清零尝试、立即到期；`done` 的事件不允许复活。
pub fn outbox_retry(conn: &Connection, id: &str, now_ms: i64) -> StorageResult<bool> {
    let updated = conn.execute(
        "UPDATE outbox_event SET state = 'pending', attempts = 0, next_retry_at_ms = ?2,
           updated_at_ms = ?2 WHERE id = ?1 AND state <> 'done'",
        params![id, now_ms],
    )?;
    Ok(updated > 0)
}

/// 队列概览：`(state, count)`。
pub fn outbox_counts(conn: &Connection) -> StorageResult<Vec<(String, i64)>> {
    let mut statement = conn.prepare("SELECT state, COUNT(*) FROM outbox_event GROUP BY state")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn map_outbox(row: &Row<'_>) -> rusqlite::Result<OutboxRecord> {
    Ok(OutboxRecord {
        id: row.get(0)?,
        dedupe_key: row.get(1)?,
        event_type: row.get(2)?,
        payload: row.get(3)?,
        state: row.get(4)?,
        attempts: row.get(5)?,
        next_retry_at_ms: row.get(6)?,
        last_error: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}

// ---------- integration_link（M5 · 外部对象回链）----------

pub fn integration_link_get(
    conn: &Connection,
    segment_id: &str,
    provider: &str,
) -> StorageResult<Option<IntegrationLinkRow>> {
    let sql = "SELECT segment_id, provider, external_id, state, last_synced_at_ms, last_error
               FROM integration_link WHERE segment_id = ?1 AND provider = ?2";
    Ok(conn
        .query_row(sql, params![segment_id, provider], map_link)
        .optional()?)
}

/// upsert 回链；`(segment_id, provider)` 主键。
pub fn integration_link_upsert(
    conn: &Connection,
    segment_id: &str,
    provider: &str,
    external_id: Option<&str>,
    state: &str,
    error: Option<&str>,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO integration_link (segment_id, provider, external_id, state, last_synced_at_ms, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(segment_id, provider) DO UPDATE SET
           external_id = excluded.external_id,
           state = excluded.state,
           last_synced_at_ms = excluded.last_synced_at_ms,
           last_error = excluded.last_error",
        params![segment_id, provider, external_id, state, now_ms, error],
    )?;
    Ok(())
}

/// 某提供方最近一次成功发布时间（集成页 `lastSyncAt`）。
pub fn integration_last_sync(conn: &Connection, provider: &str) -> StorageResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT MAX(last_synced_at_ms) FROM integration_link
             WHERE provider = ?1 AND state = 'published'",
            params![provider],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

fn map_link(row: &Row<'_>) -> rusqlite::Result<IntegrationLinkRow> {
    Ok(IntegrationLinkRow {
        segment_id: row.get(0)?,
        provider: row.get(1)?,
        external_id: row.get(2)?,
        state: row.get(3)?,
        last_synced_at_ms: row.get(4)?,
        last_error: row.get(5)?,
    })
}

/// `tags:write`：为段附加标签（不存在则创建），`revision + 1`，并写
/// `edit` 事实（携带撤销载荷）；标签名大小写不敏感。
pub fn attach_tag(
    conn: &Connection,
    segment_id: &str,
    tag_name: &str,
    now_ms: i64,
) -> StorageResult<()> {
    let old = find_segment_by_id(conn, segment_id)?
        .ok_or(StorageError::Db(rusqlite::Error::QueryReturnedNoRows))?;

    let tag_id: String = match conn
        .query_row(
            "SELECT id FROM tag WHERE name = ?1 COLLATE NOCASE",
            params![tag_name],
            |row| row.get(0),
        )
        .optional()?
    {
        Some(id) => id,
        None => {
            let id = dotlog_application::new_id();
            conn.execute(
                "INSERT INTO tag (id, name, color, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, NULL, ?3, ?3)",
                params![id, tag_name.trim(), now_ms],
            )?;
            id
        }
    };

    conn.execute(
        "INSERT OR IGNORE INTO segment_tag (segment_id, tag_id, created_at_ms) VALUES (?1, ?2, ?3)",
        params![segment_id, tag_id, now_ms],
    )?;
    conn.execute(
        "UPDATE time_segment SET revision = revision + 1, updated_at_ms = ?2 WHERE id = ?1",
        params![segment_id, now_ms],
    )?;

    let undo = dotlog_domain::EditUndo {
        restore: vec![old],
        remove_ids: Vec::new(),
    };
    let point = TimePoint::new(
        dotlog_application::new_id(),
        dotlog_domain::Instant::new(now_ms, 0),
        PointKind::Edit,
        PointSource::Plugin,
    )
    .with_segment(segment_id.to_string())
    .with_payload(undo.to_payload());
    insert_time_point(conn, &point)
}

fn note_value(note: &str) -> Option<&str> {
    if note.is_empty() {
        None
    } else {
        Some(note)
    }
}

fn point_kind_str(kind: PointKind) -> &'static str {
    match kind {
        PointKind::Start => "start",
        PointKind::Resume => "resume",
        PointKind::Punch => "punch",
        PointKind::Pause => "pause",
        PointKind::Finish => "finish",
        PointKind::Undo => "undo",
        PointKind::Delete => "delete",
        PointKind::Edit => "edit",
        PointKind::Backfill => "backfill",
    }
}

fn point_kind_from_str(value: &str) -> Option<PointKind> {
    Some(match value {
        "start" => PointKind::Start,
        "resume" => PointKind::Resume,
        "punch" => PointKind::Punch,
        "pause" => PointKind::Pause,
        "finish" => PointKind::Finish,
        "undo" => PointKind::Undo,
        "delete" => PointKind::Delete,
        "edit" => PointKind::Edit,
        "backfill" => PointKind::Backfill,
        _ => return None,
    })
}

fn point_source_str(source: PointSource) -> &'static str {
    match source {
        PointSource::Button => "button",
        PointSource::Hotkey => "hotkey",
        PointSource::Tray => "tray",
        PointSource::SecondInstance | PointSource::Api | PointSource::System => "system",
        PointSource::Plugin => "plugin",
    }
}
