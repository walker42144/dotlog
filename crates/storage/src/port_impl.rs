//! `TimerStore` 端口的 SQLite 实现：应用层不感知 SQL。

use dotlog_application::{
    AppError, AppResult, CategoryRow, IntegrationLinkRow, IntegrityReport, OutboxRecord,
    PointRecord, TimerSnapshot, TimerStore,
};
use dotlog_domain::{Effect, Segment};
use rusqlite::OptionalExtension;

use crate::db::Database;
use crate::error::StorageError;
use crate::repo;

impl TimerStore for Database {
    fn snapshot(&self) -> AppResult<TimerSnapshot> {
        self.read(|conn| {
            Ok(TimerSnapshot {
                active: repo::find_active_segment(conn)?,
                last_point_kind: if repo::paused_session(conn)? {
                    Some(dotlog_domain::PointKind::Pause)
                } else {
                    repo::last_point_kind(conn)?
                },
            })
        })
        .map_err(into_app_error)
    }

    fn commit(&self, effects: &[Effect], now_ms: i64) -> AppResult<()> {
        self.with_transaction(|tx| {
            repo::apply_effects(tx, effects, now_ms)?;
            let integrity = repo::integrity_report(tx, now_ms)?;
            if integrity.overlap_pairs != 0 {
                return Err(StorageError::Overlap);
            }
            Ok(())
        })
        .map_err(into_app_error)
    }

    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>> {
        self.read(|conn| repo::segments_overlapping(conn, from_ms, to_ms))
            .map_err(into_app_error)
    }

    fn find_segment(&self, id: &str) -> AppResult<Option<Segment>> {
        self.read(|conn| repo::find_segment_by_id(conn, id))
            .map_err(into_app_error)
    }

    fn session_segments(&self, id: &str) -> AppResult<Vec<Segment>> {
        self.read(|conn| repo::session_segments(conn, id))
            .map_err(into_app_error)
    }

    fn publication_ready(&self, id: &str) -> AppResult<bool> {
        self.read(|conn| Ok(conn.query_row("SELECT COALESCE((SELECT publish_ready FROM record_session WHERE id=(SELECT session_id FROM session_segment WHERE segment_id=?1)),1)",[id],|r|r.get(0))?)).map_err(into_app_error)
    }

    fn confirm_session(&self, id: &str, now_ms: i64) -> AppResult<()> {
        self.read(|conn| { conn.execute("UPDATE record_session SET publish_ready=1,updated_at_ms=?2 WHERE id=(SELECT session_id FROM session_segment WHERE segment_id=?1)",rusqlite::params![id,now_ms])?;Ok(()) }).map_err(into_app_error)
    }

    fn pending_note_segment(&self) -> AppResult<Option<Segment>> {
        self.read(|conn| {
            let id=conn.query_row("SELECT anchor_segment_id FROM record_session WHERE status='finished' AND publish_ready=0 ORDER BY updated_at_ms ASC LIMIT 1",[],|r|r.get::<_,String>(0)).optional()?;
            match id {Some(id)=>repo::find_segment_by_id(conn,&id),None=>Ok(None)}
        }).map_err(into_app_error)
    }

    fn current_session_segments(&self) -> AppResult<Vec<Segment>> {
        self.read(repo::current_session_segments)
            .map_err(into_app_error)
    }

    fn last_point(&self) -> AppResult<Option<PointRecord>> {
        self.read(repo::latest_point)
            .map(|point| {
                point.map(|row| PointRecord {
                    id: row.id,
                    kind: row.kind,
                    occurred_at_ms: row.occurred_at_ms,
                    segment_id: row.segment_id,
                    payload: row.payload,
                })
            })
            .map_err(into_app_error)
    }

    fn history_segments(
        &self,
        from_ms: i64,
        to_ms: i64,
        text: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)> {
        self.read(|conn| {
            repo::history_segments(
                conn,
                from_ms,
                to_ms,
                text,
                i64::from(limit),
                i64::from(offset),
            )
        })
        .map(|(items, total)| (items, total as u64))
        .map_err(into_app_error)
    }

    fn categories(&self) -> AppResult<Vec<CategoryRow>> {
        self.read(repo::list_categories).map_err(into_app_error)
    }

    fn settings(&self, scope: &str) -> AppResult<Vec<(String, String)>> {
        self.read(|conn| repo::setting_rows(conn, scope))
            .map_err(into_app_error)
    }

    fn settings_upsert(
        &self,
        scope: &str,
        entries: &[(String, String)],
        now_ms: i64,
    ) -> AppResult<()> {
        self.with_transaction(|tx| {
            for (key, value) in entries {
                repo::upsert_setting(tx, scope, key, value, now_ms)?;
            }
            Ok(())
        })
        .map_err(into_app_error)
    }

    fn settings_reset(&self, scope: &str, key: Option<&str>) -> AppResult<()> {
        self.with_transaction(|tx| repo::delete_setting(tx, scope, key))
            .map_err(into_app_error)
    }

    fn backup_to(&self, path: &std::path::Path) -> AppResult<()> {
        Database::backup_to(self, path).map_err(into_app_error)
    }

    fn restore_from(&self, path: &std::path::Path, now_ms: i64) -> AppResult<()> {
        Database::restore_from(self, path, now_ms).map_err(into_app_error)
    }

    fn runtime_state(&self, key: &str) -> AppResult<Option<String>> {
        self.read(|conn| repo::runtime_get(conn, key))
            .map_err(into_app_error)
    }

    fn set_runtime_state(&self, key: &str, value: &str, now_ms: i64) -> AppResult<()> {
        self.read(|conn| repo::runtime_set(conn, key, value, now_ms))
            .map_err(into_app_error)
    }

    fn integrity(&self, now_ms: i64) -> AppResult<IntegrityReport> {
        self.read(|conn| repo::integrity_report(conn, now_ms))
            .map_err(into_app_error)
    }

    fn table_counts(&self) -> AppResult<Vec<(String, i64)>> {
        self.read(repo::table_counts).map_err(into_app_error)
    }

    fn schema_version(&self) -> AppResult<i64> {
        self.read(repo::schema_version).map_err(into_app_error)
    }

    fn outbox_enqueue(
        &self,
        dedupe_key: Option<&str>,
        event_type: &str,
        payload: &str,
        now_ms: i64,
    ) -> AppResult<bool> {
        self.read(|conn| repo::outbox_enqueue(conn, dedupe_key, event_type, payload, now_ms))
            .map_err(into_app_error)
    }

    fn outbox_take_due(&self, now_ms: i64, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        self.read(|conn| repo::outbox_take_due(conn, now_ms, limit))
            .map_err(into_app_error)
    }

    fn outbox_defer(&self, id: &str, due_ms: i64, now_ms: i64) -> AppResult<()> {
        self.read(|conn| {
            conn.execute("UPDATE outbox_event SET state='pending', next_retry_at_ms=?2, updated_at_ms=?3 WHERE id=?1", rusqlite::params![id, due_ms, now_ms])?;
            Ok(())
        }).map_err(into_app_error)
    }

    fn outbox_mark_done(&self, id: &str, now_ms: i64) -> AppResult<()> {
        self.read(|conn| repo::outbox_mark_done(conn, id, now_ms))
            .map_err(into_app_error)
    }

    fn outbox_mark_retry(
        &self,
        id: &str,
        error: &str,
        next_retry_at_ms: i64,
        now_ms: i64,
    ) -> AppResult<()> {
        self.read(|conn| repo::outbox_mark_retry(conn, id, error, next_retry_at_ms, now_ms))
            .map_err(into_app_error)
    }

    fn outbox_mark_failed(&self, id: &str, error: &str, now_ms: i64) -> AppResult<()> {
        self.read(|conn| repo::outbox_mark_failed(conn, id, error, now_ms))
            .map_err(into_app_error)
    }

    fn outbox_list(&self, state: Option<&str>, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        self.read(|conn| repo::outbox_list(conn, state, limit))
            .map_err(into_app_error)
    }

    fn outbox_retry(&self, id: &str, now_ms: i64) -> AppResult<()> {
        self.read(|conn| repo::outbox_retry(conn, id, now_ms))
            .map(|_| ())
            .map_err(into_app_error)
    }

    fn outbox_counts(&self) -> AppResult<Vec<(String, i64)>> {
        self.read(repo::outbox_counts).map_err(into_app_error)
    }

    fn integration_link_get(
        &self,
        segment_id: &str,
        provider: &str,
    ) -> AppResult<Option<IntegrationLinkRow>> {
        self.read(|conn| repo::integration_link_get(conn, segment_id, provider))
            .map_err(into_app_error)
    }

    fn integration_link_upsert(
        &self,
        segment_id: &str,
        provider: &str,
        external_id: Option<&str>,
        state: &str,
        error: Option<&str>,
        now_ms: i64,
    ) -> AppResult<()> {
        self.read(|conn| {
            repo::integration_link_upsert(
                conn,
                segment_id,
                provider,
                external_id,
                state,
                error,
                now_ms,
            )
        })
        .map_err(into_app_error)
    }

    fn integration_last_sync(&self, provider: &str) -> AppResult<Option<i64>> {
        self.read(|conn| repo::integration_last_sync(conn, provider))
            .map_err(into_app_error)
    }

    fn segment_attach_tag(&self, segment_id: &str, tag: &str, now_ms: i64) -> AppResult<()> {
        self.with_transaction(|tx| repo::attach_tag(tx, segment_id, tag, now_ms))
            .map_err(into_app_error)
    }
}

fn into_app_error(error: StorageError) -> AppError {
    AppError::store(error.code(), error.to_string())
}
