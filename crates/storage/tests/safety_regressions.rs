use dotlog_application::dto::{BackfillInput, SegmentPatch};
use dotlog_application::{Clock, TimerService, TimerStore};
use dotlog_domain::{Instant, Phase, PointSource, PunchMode};
use dotlog_storage::{repo, Database};
use serde_json::json;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
const NOW: i64 = 1_700_000_000_000;
#[derive(Clone)]
struct TestClock(Arc<AtomicI64>);
impl Clock for TestClock {
    fn now(&self) -> Instant {
        Instant::new(self.0.load(Ordering::SeqCst), 480)
    }
}
fn fixture() -> (Database, TimerService<Database, TestClock>, TestClock) {
    let db = Database::open_in_memory(NOW).unwrap();
    let clock = TestClock(Arc::new(AtomicI64::new(NOW)));
    (
        db.clone(),
        TimerService::new(db, clock.clone(), PunchMode::Stop),
        clock,
    )
}
fn input(start: i64, end: i64) -> BackfillInput {
    BackfillInput {
        start_at: start,
        end_at: end,
        title: Some("记录".into()),
        note: None,
        category_id: None,
        is_focus: false,
        is_ignored: false,
    }
}

#[test]
fn saved_mode_and_undo_window_affect_real_sqlite_service() {
    let (_, service, clock) = fixture();
    service
        .settings_patch(
            "app",
            vec![
                ("punch.mode".into(), json!("chain")),
                ("punch.undo_window_ms".into(), json!(60_000)),
            ],
        )
        .unwrap();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    service.punch(PointSource::Button).unwrap();
    assert_eq!(service.get_state().unwrap().phase, Phase::Recording);
    clock.0.store(NOW + 80_000, Ordering::SeqCst);
    service.undo().unwrap();
    assert_eq!(service.get_state().unwrap().active.unwrap().start_at, NOW);
}

#[test]
fn merge_cannot_swallow_an_unselected_middle_record() {
    let (db, service, _) = fixture();
    let mut rows = Vec::new();
    for (a, b) in [(60_000, 50_000), (40_000, 30_000), (20_000, 10_000)] {
        rows.push(service.backfill_segment(input(NOW - a, NOW - b)).unwrap());
    }
    let error = service
        .merge_segments(&[rows[0].id.clone(), rows[2].id.clone()])
        .unwrap_err();
    assert_eq!(error.code(), "E_OVERLAP");
    assert_eq!(db.integrity(NOW).unwrap().overlap_pairs, 0);
    assert_eq!(db.day_segments(NOW - 100_000, NOW).unwrap().len(), 3);
}

#[test]
fn restore_rejects_foreign_sqlite_without_changing_records() {
    let (db, service, _) = fixture();
    let kept = service
        .backfill_segment(input(NOW - 60_000, NOW - 1))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("foreign.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE foreign_table(x TEXT); PRAGMA user_version=2;")
        .unwrap();
    drop(conn);
    assert_eq!(
        db.restore_from(&path, NOW).unwrap_err().code(),
        "E_RESTORE_INVALID"
    );
    assert!(db.find_segment(&kept.id).unwrap().is_some());
}

#[test]
fn restore_rejects_tampered_migrations_before_overwrite() {
    let (db, service, _) = fixture();
    let kept = service
        .backfill_segment(input(NOW - 60_000, NOW - 1))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.db");
    db.backup_to(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE migration SET checksum='changed'", [])
        .unwrap();
    drop(conn);
    assert!(db.restore_from(&path, NOW).is_err());
    assert!(db.find_segment(&kept.id).unwrap().is_some());
}

#[test]
fn close_and_publish_intent_are_committed_together_and_undo_cancels_it() {
    let (db, service, clock) = fixture();
    service
        .settings_patch(
            "plugin:memos",
            vec![
                ("enabled".into(), json!(true)),
                ("autoPublish".into(), json!("on_close")),
            ],
        )
        .unwrap();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    service.punch(PointSource::Button).unwrap();
    let queued = db.outbox_list(None, 100).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].event_type, "memos.publish");
    clock.0.store(NOW + 61_000, Ordering::SeqCst);
    service.undo().unwrap();
    assert_eq!(db.outbox_list(None, 100).unwrap()[0].state, "failed");
}

#[test]
fn removed_plugin_cannot_generate_new_publish_intents() {
    let (db, service, _) = fixture();
    service
        .settings_patch(
            "plugin:memos",
            vec![
                ("installed".into(), json!(false)),
                ("enabled".into(), json!(true)),
                ("autoPublish".into(), json!("on_close")),
            ],
        )
        .unwrap();
    service
        .backfill_segment(input(NOW - 60_000, NOW - 1))
        .unwrap();
    assert!(db.outbox_list(None, 100).unwrap().is_empty());
}

#[test]
fn tags_round_trip_and_null_category_really_clears() {
    let (db, service, _) = fixture();
    let item = service
        .backfill_segment(input(NOW - 60_000, NOW - 1))
        .unwrap();
    let patch: SegmentPatch =
        serde_json::from_value(json!({"categoryId":"cat-work","tags":["work","test"]})).unwrap();
    let saved = service
        .update_segment(&item.id, item.revision, patch)
        .unwrap();
    assert_eq!(saved.tags.len(), 2);
    let patch: SegmentPatch = serde_json::from_value(json!({"categoryId":null,"tags":[]})).unwrap();
    let saved = service
        .update_segment(&saved.id, saved.revision, patch)
        .unwrap();
    assert!(saved.category.is_none());
    assert!(db.find_segment(&saved.id).unwrap().unwrap().tags.is_empty());
}

#[test]
fn stale_repository_update_does_not_overwrite_newer_value() {
    let (db, service, _) = fixture();
    let item = service
        .backfill_segment(input(NOW - 60_000, NOW - 1))
        .unwrap();
    let original = db.find_segment(&item.id).unwrap().unwrap();
    let mut first = original.clone();
    first.revision += 1;
    first.title = "new".into();
    db.with_transaction(|tx| repo::update_segment(tx, &first, NOW))
        .unwrap();
    let mut stale = original;
    stale.revision += 1;
    stale.title = "stale".into();
    assert_eq!(
        db.with_transaction(|tx| repo::update_segment(tx, &stale, NOW))
            .unwrap_err()
            .code(),
        "E_STALE_REVISION"
    );
    assert_eq!(db.find_segment(&item.id).unwrap().unwrap().title, "new");
}

#[test]
fn active_session_detects_sleep_without_restarting_or_overwriting_evidence() {
    let (db, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    service.begin_session().unwrap();
    clock.0.store(NOW + 6 * 60_000, Ordering::SeqCst);
    let report = service.check_session().unwrap();
    assert!(report.recovery.required);
    assert_eq!(
        db.runtime_state("heartbeat.wall_ms").unwrap().unwrap(),
        NOW.to_string()
    );
}

#[test]
fn pause_resume_accumulates_only_work_time_and_prompts_only_on_finish() {
    let (db, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 600_000, Ordering::SeqCst);
    let paused = service.pause(PointSource::Button).unwrap();
    assert!(paused.pending_note.is_none());
    assert_eq!(service.get_state().unwrap().elapsed_seconds, 600);
    clock.0.store(NOW + 1_800_000, Ordering::SeqCst);
    assert_eq!(service.get_state().unwrap().elapsed_seconds, 600);
    assert!(service.get_state().unwrap().paused);
    service.start(PointSource::Button).unwrap();
    assert_eq!(service.get_state().unwrap().elapsed_seconds, 600);
    clock.0.store(NOW + 2_400_000, Ordering::SeqCst);
    let done = service.finish(PointSource::Button).unwrap();
    assert_eq!(done.closed_segment.duration_seconds, 1200);
    assert_eq!(done.closed_segment.start_at, NOW);
    assert!(done.pending_note.is_some());
    let slices = db.session_segments(&done.closed_segment.id).unwrap();
    assert_eq!(slices.len(), 2);
    assert_eq!(
        slices
            .iter()
            .map(|s| s.duration_ms(NOW + 2_400_000))
            .sum::<i64>(),
        1_200_000
    );
    assert_eq!(db.integrity(NOW + 2_400_000).unwrap().overlap_pairs, 0);
}

#[test]
fn finishing_while_paused_never_counts_the_pause_and_can_be_undone() {
    let (_, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 600_000, Ordering::SeqCst);
    service.pause(PointSource::Button).unwrap();
    clock.0.store(NOW + 1_800_000, Ordering::SeqCst);
    let done = service.finish(PointSource::Button).unwrap();
    assert_eq!(done.closed_segment.duration_seconds, 600);
    assert!(!service.get_state().unwrap().paused);
    clock.0.store(NOW + 1_801_000, Ordering::SeqCst);
    service.undo().unwrap();
    assert!(service.get_state().unwrap().paused);
    assert_eq!(service.get_state().unwrap().elapsed_seconds, 600);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 1_861_000, Ordering::SeqCst);
    assert_eq!(
        service
            .finish(PointSource::Button)
            .unwrap()
            .closed_segment
            .duration_seconds,
        660
    );
}

#[test]
fn finishing_metadata_is_shared_by_all_work_slices_and_publishes_once() {
    let (db, service, clock) = fixture();
    service
        .settings_patch(
            "plugin:memos",
            vec![
                ("enabled".into(), json!(true)),
                ("autoPublish".into(), json!("on_close")),
            ],
        )
        .unwrap();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    service.pause(PointSource::Button).unwrap();
    assert!(db.outbox_list(None, 100).unwrap().is_empty());
    clock.0.store(NOW + 120_000, Ordering::SeqCst);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 180_000, Ordering::SeqCst);
    let done = service.finish(PointSource::Button).unwrap();
    clock.0.store(NOW + 181_000, Ordering::SeqCst);
    service
        .update_segment(
            &done.closed_segment.id,
            done.closed_segment.revision,
            SegmentPatch {
                title: Some("学习英语".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let slices = db.session_segments(&done.closed_segment.id).unwrap();
    assert!(slices.iter().all(|s| s.title == "学习英语"));
    assert_eq!(db.outbox_list(None, 100).unwrap().len(), 1);
}

#[test]
fn paused_session_survives_restart_and_metadata_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.db");
    let clock = TestClock(Arc::new(AtomicI64::new(NOW)));
    let db = Database::open(&path, NOW).unwrap();
    let service = TimerService::new(db.clone(), clock.clone(), PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    let paused = service.pause(PointSource::Button).unwrap();
    clock.0.store(NOW + 61_000, Ordering::SeqCst);
    service
        .update_segment(
            &paused.closed_segment.id,
            paused.closed_segment.revision,
            SegmentPatch {
                title: Some("临时改标题".into()),
                ..Default::default()
            },
        )
        .unwrap();
    drop(service);
    drop(db);
    let reopened = Database::open(&path, NOW + 120_000).unwrap();
    let service = TimerService::new(reopened, clock, PunchMode::Stop);
    let state = service.get_state().unwrap();
    assert!(state.paused);
    assert_eq!(state.elapsed_seconds, 60);
}

#[test]
fn undo_resume_returns_to_paused_session_instead_of_losing_elapsed_time() {
    let (_, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    service.pause(PointSource::Button).unwrap();
    clock.0.store(NOW + 120_000, Ordering::SeqCst);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 121_000, Ordering::SeqCst);
    service.undo().unwrap();
    let state = service.get_state().unwrap();
    assert!(state.paused);
    assert_eq!(state.elapsed_seconds, 60);
}

#[test]
fn multiple_pauses_exclude_every_gap_and_keep_a_single_logical_record() {
    let (db, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    service.pause(PointSource::Button).unwrap();
    clock.0.store(NOW + 600_000, Ordering::SeqCst);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 720_000, Ordering::SeqCst);
    service.pause(PointSource::Button).unwrap();
    clock.0.store(NOW + 1_800_000, Ordering::SeqCst);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 1_980_000, Ordering::SeqCst);
    let done = service.finish(PointSource::Button).unwrap();
    assert_eq!(done.closed_segment.duration_seconds, 360);
    assert_eq!(
        db.session_segments(&done.closed_segment.id).unwrap().len(),
        3
    );
    assert_eq!(service.day(None).unwrap().stats.recorded_seconds, 360);
}

#[test]
fn automatic_publication_waits_for_finish_note_confirmation_even_after_restart() {
    let (db, service, clock) = fixture();
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    let done = service.finish(PointSource::Button).unwrap();
    assert!(!db.publication_ready(&done.closed_segment.id).unwrap());
    assert_eq!(
        service.pending_note().unwrap().unwrap().id,
        done.closed_segment.id
    );
    service.confirm_session(&done.closed_segment.id).unwrap();
    assert!(db.publication_ready(&done.closed_segment.id).unwrap());
    assert!(service.pending_note().unwrap().is_none());
}

#[test]
fn upgrading_v2_preserves_existing_record_and_paused_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let clock = TestClock(Arc::new(AtomicI64::new(NOW)));
    let db = Database::open(&path, NOW).unwrap();
    let service = TimerService::new(db.clone(), clock.clone(), PunchMode::Stop);
    service.start(PointSource::Button).unwrap();
    clock.0.store(NOW + 60_000, Ordering::SeqCst);
    let old = service.pause(PointSource::Button).unwrap();
    drop(service);
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE session_segment; DROP TABLE record_session; DELETE FROM migration WHERE version=3; PRAGMA user_version=2;").unwrap();
    drop(conn);
    let db = Database::open(&path, NOW + 120_000).unwrap();
    assert!(db.find_segment(&old.closed_segment.id).unwrap().is_some());
    let service = TimerService::new(db, clock, PunchMode::Stop);
    let state = service.get_state().unwrap();
    assert!(state.paused);
    assert_eq!(state.elapsed_seconds, 60);
}

#[test]
fn restored_local_plugins_are_disabled_and_keep_their_data() {
    let (db,_,_)=fixture();
    db.settings_upsert("local-plugins", &[("registry".into(), json!({"local-demo":{"package":{"id":"local-demo"},"enabled":true,"showInSidebar":true}}).to_string()),("data:local-demo".into(), json!({"note":"保留复盘"}).to_string())], NOW).unwrap();
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("plugins.db");db.backup_to(&path).unwrap();
    db.restore_from(&path,NOW).unwrap();
    let rows=db.settings("local-plugins").unwrap();let value:serde_json::Value=serde_json::from_str(&rows.iter().find(|(key,_)|key=="registry").unwrap().1).unwrap();
    assert_eq!(value["local-demo"]["enabled"],false);
    assert!(rows.iter().find(|(key,_)|key=="data:local-demo").unwrap().1.contains("保留复盘"));
}
