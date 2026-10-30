//! Reproducible stage-two demonstration using SQLite in memory only.
use dotlog_application::tomatodo_link::{
    LinkCommand as Cmd, Observation, SourcePhase, SourceSession,
};
use dotlog_application::{Clock, TimerService, TimerStore};
use dotlog_domain::{Instant, PunchMode};
use dotlog_storage::Database;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct DemoClock(Arc<Mutex<i64>>);
impl Clock for DemoClock {
    fn now(&self) -> Instant {
        Instant::new(*self.0.lock().unwrap(), 480)
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t = 1_700_000_000_000;
    let clock = DemoClock(Arc::new(Mutex::new(t)));
    let db = Database::open_in_memory(t)?;
    let service = TimerService::new(db.clone(), clock.clone(), PunchMode::Chain);
    let sample = |phase| {
        Cmd::Observe(Observation {
            phase,
            session: Some(SourceSession {
                key: "demo:linear-algebra:1".into(),
                title: "线代".into(),
                started_at_ms: t + 1000,
            }),
            count_up: true,
            observed_at_ms: clock.now().utc_ms,
        })
    };
    service.tomatodo_link(sample(SourcePhase::Idle))?;
    *clock.0.lock().unwrap() = t + 1000;
    let first = service
        .tomatodo_link(sample(SourcePhase::Focus))?
        .active_segment_id
        .unwrap();
    for second in 2..=61 {
        *clock.0.lock().unwrap() = t + second * 1000;
        service.tomatodo_link(sample(SourcePhase::Focus))?;
    }
    service.tomatodo_link(Cmd::EndSegment)?;
    *clock.0.lock().unwrap() = t + 62_000;
    service.tomatodo_link(sample(SourcePhase::Focus))?;
    assert!(db.snapshot()?.active.is_none());
    service.tomatodo_link(Cmd::StartNext)?;
    for second in 63..=122 {
        *clock.0.lock().unwrap() = t + second * 1000;
        service.tomatodo_link(sample(SourcePhase::Focus))?;
    }
    *clock.0.lock().unwrap() = t + 123_000;
    let ended = service.tomatodo_link(sample(SourcePhase::Rest))?;
    assert_eq!(ended.pending_notes.len(), 2);
    let revision = db.find_segment(&first)?.unwrap().revision;
    service.tomatodo_link(Cmd::SaveNote {
        segment_id: first.clone(),
        revision,
        note: "完成矩阵例题，订正两处计算错误".into(),
    })?;
    *clock.0.lock().unwrap() = t + 124_000;
    let context = service.tomatodo_link(sample(SourcePhase::Idle))?;
    let records = db.day_segments(t, t + 200_000)?;
    assert_eq!(records.len(), 2);
    assert!(db.snapshot()?.active.is_none());
    assert!(!context.gap_detected);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "database":"SQLite in memory only", "realUserRecordsTouched":false,
            "scenario":"开始→结束第一段→保持停止→手动开始第二段→番茄手动结束进入休息→跳过休息→补第一段备注",
            "recordCount":records.len(),"pendingNotes":context.pending_notes.len(),
            "sourceAssociationCleared":context.session.is_none(),
            "gapDetected":context.gap_detected,
            "records":records.iter().map(|s|serde_json::json!({"title":s.title,"durationSeconds":s.duration_ms(t+200_000)/1000,"note":s.note})).collect::<Vec<_>>()
        }))?
    );
    Ok(())
}
