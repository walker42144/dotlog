//! All writes use in-memory/temporary SQLite, never the user's application data.
use dotlog_application::tomatodo_link::{
    LinkCommand as Cmd, Observation, SourcePhase as Phase, SourceSession, RUNTIME_KEY,
};
use dotlog_application::{Clock, TimerService, TimerStore};
use dotlog_domain::{Instant, PointSource, PunchMode};
use dotlog_storage::Database;
use std::sync::{Arc, Mutex};

const T: i64 = 1_700_000_000_000;
#[derive(Clone)]
struct TestClock(Arc<Mutex<i64>>);
impl Clock for TestClock {
    fn now(&self) -> Instant {
        Instant::new(*self.0.lock().unwrap(), 480)
    }
}
struct Fixture {
    db: Database,
    clock: TestClock,
    service: TimerService<Database, TestClock>,
}
impl Fixture {
    fn new() -> Self {
        let db = Database::open_in_memory(T).unwrap();
        let clock = TestClock(Arc::new(Mutex::new(T)));
        let service = TimerService::new(db.clone(), clock.clone(), PunchMode::Chain);
        Self { db, clock, service }
    }
    fn tick(&self, n: i64) {
        *self.clock.0.lock().unwrap() += n;
    }
    fn observe(&self, phase: Phase, started: i64, title: &str) -> Cmd {
        Cmd::Observe(Observation {
            phase,
            session: matches!(phase, Phase::Focus | Phase::Paused).then(|| SourceSession {
                key: format!("same-task:{started}"),
                title: title.into(),
                started_at_ms: started,
            }),
            observed_at_ms: self.clock.now().utc_ms,
            count_up: true,
        })
    }
    fn idle(&self) {
        self.service
            .tomatodo_link(self.observe(Phase::Idle, T, "数学"))
            .unwrap();
    }
    fn begin(&self) -> String {
        self.idle();
        self.tick(1000);
        self.service
            .tomatodo_link(self.observe(Phase::Focus, T + 1000, "数学"))
            .unwrap()
            .active_segment_id
            .unwrap()
    }
}

#[test]
fn two_segments_keep_title_and_pending_notes_without_automatic_reopen() {
    let f = Fixture::new();
    let first = f.begin();
    f.tick(2000);
    let stopped = f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    assert!(stopped.manually_stopped);
    assert!(stopped.active_segment_id.is_none());
    for _ in 0..5 {
        f.tick(100);
        f.service
            .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
            .unwrap();
    }
    assert!(f.db.snapshot().unwrap().active.is_none());
    let second = f
        .service
        .tomatodo_link(Cmd::StartNext)
        .unwrap()
        .active_segment_id
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(f.db.find_segment(&second).unwrap().unwrap().title, "数学");
    f.tick(2000);
    let ended = f
        .service
        .tomatodo_link(f.observe(Phase::Rest, T, "数学"))
        .unwrap();
    assert_eq!(ended.pending_notes, vec![first.clone(), second.clone()]);
    assert!(ended.session.is_none());
    assert!(!f.db.publication_ready(&first).unwrap());
    f.tick(1000);
    f.idle();
    f.tick(1000);
    f.idle();
    assert_eq!(f.db.day_segments(T, T + 10000).unwrap().len(), 2);
    let s = f.db.find_segment(&first).unwrap().unwrap();
    let queue = f
        .service
        .tomatodo_link(Cmd::SaveNote {
            segment_id: first.clone(),
            revision: s.revision,
            note: "完成例题一".into(),
        })
        .unwrap();
    assert_eq!(queue.pending_notes, vec![second]);
    assert_eq!(
        f.db.find_segment(&first).unwrap().unwrap().note,
        "完成例题一"
    );
    assert!(f.db.publication_ready(&first).unwrap());
}

#[test]
fn source_end_while_manually_stopped_creates_no_empty_segment() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Rest, T, ""))
        .unwrap();
    assert_eq!(c.pending_notes, vec![id]);
    assert!(c.session.is_none());
    assert_eq!(f.db.day_segments(T, T + 10000).unwrap().len(), 1);
}

#[test]
fn new_same_title_session_starts_again_but_old_replay_does_not() {
    let f = Fixture::new();
    let first = f.begin();
    f.tick(1000);
    f.service
        .tomatodo_link(f.observe(Phase::Rest, T, ""))
        .unwrap();
    f.tick(1000);
    let replay = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(replay.active_segment_id.is_none());
    f.tick(1000);
    let next = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 4000, "数学"))
        .unwrap();
    assert_ne!(next.active_segment_id.unwrap(), first);
}

#[test]
fn pause_and_resume_exclude_the_gap_but_manual_stop_still_wins() {
    let f = Fixture::new();
    f.begin();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Paused, T + 1000, "数学"))
        .unwrap();
    assert!(c.resume_on_focus);
    assert!(c.active_segment_id.is_none());
    f.tick(3000);
    f.service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    f.tick(100);
    f.service
        .tomatodo_link(f.observe(Phase::Paused, T + 1000, "数学"))
        .unwrap();
    f.tick(100);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.active_segment_id.is_none());
    let spans = f.db.day_segments(T, T + 10000).unwrap();
    assert_eq!(
        spans.iter().map(|s| s.duration_ms(T + 10000)).sum::<i64>(),
        2000
    );
}

#[test]
fn unrelated_active_or_paused_record_is_not_taken_over() {
    let f = Fixture::new();
    f.idle();
    let manual = f
        .service
        .start(PointSource::Button)
        .unwrap()
        .active
        .unwrap();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.conflict.is_some());
    assert!(c.active_segment_id.is_none());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, manual.id);
    assert!(f.service.tomatodo_link(Cmd::StartNext).is_err());
    f.tick(1000);
    f.service.pause(PointSource::Button).unwrap();
    assert!(f.service.tomatodo_link(Cmd::StartNext).is_err());
}

#[test]
fn attaching_mid_session_requires_explicit_start_and_no_backfill() {
    let f = Fixture::new();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T, "数学"))
        .unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.conflict.is_some());
    let id = f
        .service
        .tomatodo_link(Cmd::StartNext)
        .unwrap()
        .active_segment_id
        .unwrap();
    assert_eq!(
        f.db.find_segment(&id).unwrap().unwrap().start_at_ms,
        T + 1000
    );
}

#[test]
fn reconnect_same_session_preserves_owned_id_without_duplicate_start() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Unknown, T, ""))
        .unwrap();
    assert!(!c.connected);
    assert!(c.gap_detected);
    assert!(f.service.tomatodo_link(Cmd::StartNext).is_err());
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert_eq!(c.active_segment_id, Some(id));
    assert!(c.gap_detected);
    assert_eq!(f.db.day_segments(T, T + 10000).unwrap().len(), 1);
}

#[test]
fn end_or_session_switch_after_disconnect_requires_reconciliation() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service
        .tomatodo_link(f.observe(Phase::Unknown, T, ""))
        .unwrap();
    f.tick(1000);
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Rest, T, ""))
        .is_err());
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 3000, "英语"))
        .is_err());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, id);
}

#[test]
fn stale_out_of_order_countdown_and_changed_identity_are_rejected() {
    let f = Fixture::new();
    let id = f.begin();
    let mut o = Observation {
        phase: Phase::Focus,
        session: Some(SourceSession {
            key: format!("same-task:{}", T + 1000),
            title: "数学".into(),
            started_at_ms: T + 1000,
        }),
        observed_at_ms: T + 1000,
        count_up: false,
    };
    assert!(f.service.tomatodo_link(Cmd::Observe(o.clone())).is_err());
    o.count_up = true;
    o.observed_at_ms = T;
    assert!(f.service.tomatodo_link(Cmd::Observe(o.clone())).is_err());
    o.observed_at_ms = T + 1000;
    o.session.as_mut().unwrap().started_at_ms = T;
    assert!(f.service.tomatodo_link(Cmd::Observe(o)).is_err());
    f.tick(6000);
    assert!(f.service.tomatodo_link(Cmd::StartNext).is_err());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, id);
}

#[test]
fn restarting_service_restores_binding_and_all_pending_notes() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let restarted = TimerService::new(f.db.clone(), f.clock.clone(), PunchMode::Stop);
    let c = restarted.tomatodo_link_state().unwrap();
    assert_eq!(c.pending_notes, vec![id]);
    assert!(c.manually_stopped);
    f.tick(1000);
    let c = restarted
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.session.is_some());
}

#[test]
fn note_revision_conflict_preserves_queue_and_skip_preserves_existing_text() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let s = f.db.find_segment(&id).unwrap().unwrap();
    assert!(f
        .service
        .tomatodo_link(Cmd::SaveNote {
            segment_id: id.clone(),
            revision: s.revision + 1,
            note: "bad".into()
        })
        .is_err());
    assert_eq!(
        f.service.tomatodo_link_state().unwrap().pending_notes,
        vec![id.clone()]
    );
    f.service
        .update_segment(
            &id,
            s.revision,
            dotlog_application::dto::SegmentPatch {
                note: Some("已有备注".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let s = f.db.find_segment(&id).unwrap().unwrap();
    let c = f
        .service
        .tomatodo_link(Cmd::SkipNote {
            segment_id: id.clone(),
            revision: s.revision,
        })
        .unwrap();
    assert!(c.pending_notes.is_empty());
    assert_eq!(f.db.find_segment(&id).unwrap().unwrap().note, "已有备注");
}

#[test]
fn atomic_failure_rolls_back_timer_and_runtime_context() {
    let f = Fixture::new();
    f.idle();
    let before = f.db.runtime_state(RUNTIME_KEY).unwrap();
    f.db.with_transaction(|tx| { tx.execute_batch("CREATE TRIGGER reject_link BEFORE UPDATE ON runtime_state WHEN NEW.key='tomatodo.link.v1' BEGIN SELECT RAISE(ABORT,'injected'); END;")?; Ok(()) }).unwrap();
    f.tick(1000);
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .is_err());
    assert!(f.db.snapshot().unwrap().active.is_none());
    assert_eq!(f.db.runtime_state(RUNTIME_KEY).unwrap(), before);
    assert!(f.db.day_segments(T, T + 10000).unwrap().is_empty());
}

#[test]
fn corrupted_context_is_not_silently_reset() {
    let f = Fixture::new();
    f.db.set_runtime_state(RUNTIME_KEY, "not json", T).unwrap();
    assert!(f.service.tomatodo_link_state().is_err());
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Idle, T, ""))
        .is_err());
}

#[test]
fn missed_heartbeat_is_a_gap_even_without_explicit_disconnect() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(10_000);
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Rest, T, ""))
        .is_err());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, id);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.gap_detected);
    assert_eq!(c.active_segment_id, Some(id));
}

#[test]
fn changing_title_only_affects_new_segments() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "线代"))
        .unwrap();
    assert_eq!(f.db.find_segment(&id).unwrap().unwrap().title, "数学");
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let next = f
        .service
        .tomatodo_link(Cmd::StartNext)
        .unwrap()
        .active_segment_id
        .unwrap();
    assert_eq!(f.db.find_segment(&next).unwrap().unwrap().title, "线代");
}

#[test]
fn pending_link_note_blocks_publication_even_with_global_prompt_off() {
    let f = Fixture::new();
    f.db.with_transaction(|tx| {
        tx.execute("INSERT OR REPLACE INTO setting(scope,key,value,updated_at_ms) VALUES('app','notify.on_punch','false',?1)",[T])?;
        Ok(())
    }).unwrap();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    assert!(!f.db.publication_ready(&id).unwrap());
    let s = f.db.find_segment(&id).unwrap().unwrap();
    f.service
        .tomatodo_link(Cmd::SkipNote {
            segment_id: id.clone(),
            revision: s.revision,
        })
        .unwrap();
    assert!(f.db.publication_ready(&id).unwrap());
}

#[test]
fn ordinary_timer_operation_cannot_silently_replace_owned_segment() {
    let f = Fixture::new();
    f.begin();
    f.tick(1000);
    f.service.finish(PointSource::Button).unwrap();
    f.tick(1000);
    let unrelated = f
        .service
        .start(PointSource::Button)
        .unwrap()
        .active
        .unwrap();
    assert!(f
        .service
        .tomatodo_link(f.observe(Phase::Rest, T, ""))
        .is_err());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, unrelated.id);
}

#[test]
fn file_reopen_keeps_link_and_note_queue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("isolated.db");
    let clock = TestClock(Arc::new(Mutex::new(T)));
    let db = Database::open(&path, T).unwrap();
    let svc = TimerService::new(db, clock.clone(), PunchMode::Stop);
    let observe = |phase| {
        Cmd::Observe(Observation {
            phase,
            session: Some(SourceSession {
                key: "one".into(),
                title: "数学".into(),
                started_at_ms: T,
            }),
            observed_at_ms: clock.now().utc_ms,
            count_up: true,
        })
    };
    svc.tomatodo_link(observe(Phase::Idle)).unwrap();
    *clock.0.lock().unwrap() += 1000;
    svc.tomatodo_link(observe(Phase::Focus)).unwrap();
    *clock.0.lock().unwrap() += 1000;
    let queue = svc.tomatodo_link(Cmd::EndSegment).unwrap().pending_notes;
    drop(svc);
    let svc = TimerService::new(
        Database::open(&path, T + 2000).unwrap(),
        clock,
        PunchMode::Stop,
    );
    assert_eq!(svc.tomatodo_link_state().unwrap().pending_notes, queue);
}

#[test]
fn fast_skip_starts_new_session_and_marks_old_boundary_for_review() {
    let f = Fixture::new();
    let old = f.begin();
    f.tick(1000);
    f.service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 3000, "英语"))
        .unwrap();
    assert!(c.gap_detected);
    assert!(c.conflict.is_some());
    assert_ne!(c.active_segment_id.as_deref(), Some(old.as_str()));
    assert_eq!(
        f.db.find_segment(&old).unwrap().unwrap().end_at_ms,
        Some(T + 2000)
    );
    assert!(c.pending_notes.contains(&old));
}

#[test]
fn ordinary_edit_and_confirm_cannot_bypass_pending_link_notes() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let s = f.db.find_segment(&id).unwrap().unwrap();
    f.service
        .update_segment(
            &id,
            s.revision,
            dotlog_application::dto::SegmentPatch {
                is_focus: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!f.db.publication_ready(&id).unwrap());
    assert!(f.db.confirm_session(&id, T + 2000).is_err());
    assert!(f
        .service
        .tomatodo_link_state()
        .unwrap()
        .pending_notes
        .contains(&id));
}

#[test]
fn unchanged_samples_do_not_write_sqlite() {
    let f = Fixture::new();
    f.begin();
    let count = || {
        f.db.read(|c| Ok(c.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    };
    let before = count();
    for _ in 0..100 {
        f.tick(10);
        f.service
            .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
            .unwrap();
    }
    assert_eq!(count(), before);
}

#[test]
fn initial_connection_failures_do_not_prevent_first_valid_baseline() {
    let f = Fixture::new();
    f.service
        .tomatodo_link(f.observe(Phase::Unknown, T, ""))
        .unwrap();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.session.is_some());
    assert!(c.active_segment_id.is_none());
    assert!(f
        .service
        .tomatodo_link(Cmd::StartNext)
        .unwrap()
        .active_segment_id
        .is_some());
}

#[test]
fn deleted_pending_record_can_be_explicitly_removed_from_queue() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    f.service.delete_segment(&id).unwrap();
    let c = f
        .service
        .tomatodo_link(Cmd::SkipNote {
            segment_id: id,
            revision: 0,
        })
        .unwrap();
    assert!(c.pending_notes.is_empty());
}

#[test]
fn clock_rollback_reports_review_instead_of_rejecting_disconnect_forever() {
    let f = Fixture::new();
    f.begin();
    *f.clock.0.lock().unwrap() -= 60_000;
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Unknown, T, ""))
        .unwrap();
    assert!(!c.connected);
    assert!(c.conflict.unwrap().contains("时间回拨"));
    assert!(c.gap_detected);
}

#[test]
fn disconnected_end_can_be_reconciled_and_detached_without_losing_notes() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(1000);
    f.service
        .tomatodo_link(f.observe(Phase::Unknown, T, ""))
        .unwrap();
    f.tick(1000);
    let c = f
        .service
        .tomatodo_link(Cmd::Reconcile {
            end_at_ms: T + 2000,
        })
        .unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.session.is_none());
    assert_eq!(c.pending_notes, vec![id]);
    assert!(!f.service.tomatodo_link(Cmd::Detach).unwrap().initialized);
}

#[test]
fn ordinary_recovery_closes_link_atomically_and_preserves_note_gate() {
    use dotlog_domain::{RecoveryAction, RecoveryReason};
    for action in [
        RecoveryAction::Extend,
        RecoveryAction::Truncate,
        RecoveryAction::Discard,
    ] {
        let f = Fixture::new();
        let id = f.begin();
        f.tick(2000);
        f.service
            .resolve_recovery(RecoveryReason::LongGap, action, Some(T + 2000))
            .unwrap();
        let c = f.service.tomatodo_link_state().unwrap();
        assert!(c.active_segment_id.is_none());
        assert!(c.session.is_none());
        assert_eq!(
            c.pending_notes.contains(&id),
            action != RecoveryAction::Discard
        );
        if action != RecoveryAction::Discard {
            assert!(!f.db.publication_ready(&id).unwrap());
        }
        let persisted: dotlog_application::tomatodo_link::LinkState =
            serde_json::from_str(&f.db.runtime_state(RUNTIME_KEY).unwrap().unwrap()).unwrap();
        assert_eq!(persisted, c);
        f.service.tomatodo_link(Cmd::Detach).unwrap();
    }
}
#[test]
fn ordinary_delete_clears_active_link_and_allows_fresh_baseline() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(2000);
    f.service.delete_segment(&id).unwrap();
    let c = f.service.tomatodo_link_state().unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.pending_notes.is_empty());
    let c = f
        .service
        .tomatodo_link(f.observe(Phase::Focus, T + 1000, "数学"))
        .unwrap();
    assert!(c.session.is_some());
    assert!(c.active_segment_id.is_none());
    f.service.tomatodo_link(Cmd::StartNext).unwrap();
}
#[test]
fn reconcile_legacy_closed_pointer_does_not_touch_unrelated_active_segment() {
    let f = Fixture::new();
    let old = f.begin();
    f.tick(2000);
    f.service.finish(PointSource::Button).unwrap();
    f.tick(1000);
    f.service.start(PointSource::Button).unwrap();
    let unrelated = f.db.snapshot().unwrap().active.unwrap().id;
    let c = f
        .service
        .tomatodo_link(Cmd::Reconcile {
            end_at_ms: f.clock.now().utc_ms,
        })
        .unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.pending_notes.contains(&old));
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, unrelated);
    assert!(!f.db.publication_ready(&old).unwrap());
    f.service.tomatodo_link(Cmd::Detach).unwrap();
}
#[test]
fn reconcile_legacy_deleted_pointer_can_detach() {
    let f = Fixture::new();
    let id = f.begin();
    f.tick(2000);
    // Model a pre-fix deletion that changed storage but not the persisted pointer.
    let s =
        f.db.snapshot()
            .unwrap()
            .active
            .unwrap()
            .closed(f.clock.now())
            .unwrap();
    f.db.commit(
        &[
            dotlog_domain::Effect::UpdateSegment(s),
            dotlog_domain::Effect::SoftDeleteSegment {
                id: id.clone(),
                at_ms: f.clock.now().utc_ms,
            },
        ],
        f.clock.now().utc_ms,
    )
    .unwrap();
    let c = f
        .service
        .tomatodo_link(Cmd::Reconcile {
            end_at_ms: f.clock.now().utc_ms,
        })
        .unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(!c.pending_notes.contains(&id));
    f.service.tomatodo_link(Cmd::Detach).unwrap();
}
#[test]
fn pending_notes_block_merge_and_split_until_explicit_confirmation() {
    let f = Fixture::new();
    let first = f.begin();
    f.tick(2000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let second = f
        .service
        .tomatodo_link(Cmd::StartNext)
        .unwrap()
        .active_segment_id
        .unwrap();
    f.tick(2000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    assert!(f
        .service
        .merge_segments(&[first.clone(), second.clone()])
        .is_err());
    assert!(f.service.split_segment(&first, T + 2000).is_err());
    assert!(!f.db.publication_ready(&first).unwrap());
    for id in [&first, &second] {
        let s = f.db.find_segment(id).unwrap().unwrap();
        f.service
            .tomatodo_link(Cmd::SkipNote {
                segment_id: id.clone(),
                revision: s.revision,
            })
            .unwrap();
    }
    assert!(f.service.merge_segments(&[first, second]).is_ok());
}

#[test]
fn recovery_link_transaction_failure_keeps_both_segment_and_context() {
    use dotlog_domain::{RecoveryAction, RecoveryReason};
    let f = Fixture::new();
    let id = f.begin();
    f.tick(2000);
    let before = f.service.tomatodo_link_state().unwrap();
    f.db.read(|c| {c.execute_batch("CREATE TRIGGER reject_link_update BEFORE UPDATE ON runtime_state WHEN NEW.key='tomatodo.link.v1' BEGIN SELECT RAISE(ABORT,'test rollback'); END;")?; Ok(())}).unwrap();
    assert!(f
        .service
        .resolve_recovery(RecoveryReason::OrphanActive, RecoveryAction::Extend, None)
        .is_err());
    assert_eq!(f.db.snapshot().unwrap().active.unwrap().id, id);
    assert_eq!(f.service.tomatodo_link_state().unwrap(), before);
    let persisted: dotlog_application::tomatodo_link::LinkState =
        serde_json::from_str(&f.db.runtime_state(RUNTIME_KEY).unwrap().unwrap()).unwrap();
    assert_eq!(persisted.active_segment_id, Some(id));
}

#[test]
fn automatic_attachment_records_current_focus_from_now_and_respects_manual_stop() {
    let f = Fixture::new();
    f.tick(4000);
    let Cmd::Observe(o) = f.observe(Phase::Focus, T + 1000, "线代") else {
        unreachable!()
    };
    let c = f.service.tomatodo_link(Cmd::ObserveAutomatic(o)).unwrap();
    let id = c.active_segment_id.unwrap();
    assert_eq!(
        f.db.find_segment(&id).unwrap().unwrap().start_at_ms,
        T + 4000
    );
    f.tick(1000);
    f.service.tomatodo_link(Cmd::EndSegment).unwrap();
    let Cmd::Observe(o) = f.observe(Phase::Focus, T + 1000, "线代") else {
        unreachable!()
    };
    assert!(f
        .service
        .tomatodo_link(Cmd::ObserveAutomatic(o))
        .unwrap()
        .active_segment_id
        .is_none());
}
#[test]
fn automatic_attachment_after_idle_gap_never_backfills_or_takes_unrelated_timer() {
    let f = Fixture::new();
    f.idle();
    f.tick(10000);
    let Cmd::Observe(o) = f.observe(Phase::Focus, T + 6000, "线代") else {
        unreachable!()
    };
    let c = f.service.tomatodo_link(Cmd::ObserveAutomatic(o)).unwrap();
    assert_eq!(
        f.db.find_segment(&c.active_segment_id.unwrap())
            .unwrap()
            .unwrap()
            .start_at_ms,
        T + 10000
    );
    let g = Fixture::new();
    g.service.start(PointSource::Button).unwrap();
    g.tick(2000);
    let unrelated = g.db.snapshot().unwrap().active.unwrap().id;
    let Cmd::Observe(o) = g.observe(Phase::Focus, T + 1000, "线代") else {
        unreachable!()
    };
    let c = g.service.tomatodo_link(Cmd::ObserveAutomatic(o)).unwrap();
    assert!(c.active_segment_id.is_none());
    assert_eq!(g.db.snapshot().unwrap().active.unwrap().id, unrelated);
}
#[test]
fn automatic_attachment_keeps_explicit_reconciliation_stopped() {
    let f = Fixture::new();
    f.begin();
    f.tick(2000);
    f.service
        .tomatodo_link(Cmd::Reconcile {
            end_at_ms: f.clock.now().utc_ms,
        })
        .unwrap();
    let Cmd::Observe(o) = f.observe(Phase::Focus, T + 1000, "线代") else {
        unreachable!()
    };
    assert!(f
        .service
        .tomatodo_link(Cmd::ObserveAutomatic(o))
        .unwrap()
        .active_segment_id
        .is_none());
}

#[test]
fn automatic_attachment_waits_for_source_resume_without_counting_pause() {
    let f = Fixture::new();
    f.tick(2000);
    let Cmd::Observe(o) = f.observe(Phase::Paused, T + 1000, "线代") else {
        unreachable!()
    };
    let c = f.service.tomatodo_link(Cmd::ObserveAutomatic(o)).unwrap();
    assert!(c.active_segment_id.is_none());
    assert!(c.resume_on_focus);
    f.tick(3000);
    let Cmd::Observe(o) = f.observe(Phase::Focus, T + 1000, "线代") else {
        unreachable!()
    };
    let c = f.service.tomatodo_link(Cmd::ObserveAutomatic(o)).unwrap();
    assert_eq!(
        f.db.find_segment(&c.active_segment_id.unwrap())
            .unwrap()
            .unwrap()
            .start_at_ms,
        T + 5000
    );
}
