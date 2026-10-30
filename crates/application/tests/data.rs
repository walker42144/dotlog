//! 应用层数据用例测试：统计、设置、导出与导入（内存假存储）。

use std::sync::{Arc, Mutex};

use dotlog_application::{
    AppResult, CategoryRow, Clock, ExportFormat, Granularity, PointRecord, TimerService,
    TimerSnapshot, TimerStore,
};
use dotlog_domain::{Effect, Instant, PunchMode, Segment};

const T0: i64 = 1_793_000_000_000; // 2026-10-26T15:33:20+08:00
const TZ: i32 = 480;

#[derive(Default)]
struct FakeData {
    segments: Vec<Segment>,
    points: Vec<PointRecord>,
    categories: Vec<CategoryRow>,
    settings: Vec<(String, String, String)>,
}

#[derive(Clone, Default)]
struct FakeStore {
    inner: Arc<Mutex<FakeData>>,
}

impl FakeStore {
    fn with_categories(categories: &[(&str, &str, &str)]) -> Self {
        let store = Self::default();
        store.inner.lock().unwrap().categories = categories
            .iter()
            .map(|(id, name, color)| CategoryRow {
                id: (*id).to_string(),
                name: (*name).to_string(),
                color: (*color).to_string(),
                parent_id: String::new(),
            })
            .collect();
        store
    }

    fn push_segment(&self, segment: Segment) {
        self.inner.lock().unwrap().segments.push(segment);
    }
}

impl TimerStore for FakeStore {
    fn snapshot(&self) -> AppResult<TimerSnapshot> {
        Ok(TimerSnapshot::default())
    }

    fn commit(&self, effects: &[Effect], _now_ms: i64) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for effect in effects {
            match effect {
                Effect::InsertSegment(segment) => data.segments.push(segment.clone()),
                Effect::UpdateSegment(segment) => {
                    data.segments.retain(|stored| stored.id != segment.id);
                    data.segments.push(segment.clone());
                }
                Effect::SoftDeleteSegment { id, .. } => {
                    data.segments.retain(|stored| &stored.id != id)
                }
                Effect::RestoreSegment(segment) => {
                    data.segments.retain(|stored| stored.id != segment.id);
                    data.segments.push(segment.clone());
                }
                Effect::InsertTimePoint(point) => data.points.push(PointRecord {
                    id: point.id.clone(),
                    kind: point.kind,
                    occurred_at_ms: point.occurred_at_ms,
                    segment_id: point.segment_id.clone(),
                    payload: point.payload.clone(),
                }),
                Effect::MarkPointUndone { .. } => {}
            }
        }
        Ok(())
    }

    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>> {
        let data = self.inner.lock().unwrap();
        let mut segments: Vec<Segment> = data
            .segments
            .iter()
            .filter(|segment| {
                let end = segment.end_at_ms.unwrap_or(i64::MAX);
                segment.start_at_ms < to_ms && end > from_ms
            })
            .cloned()
            .collect();
        segments.sort_by_key(|segment| segment.start_at_ms);
        Ok(segments)
    }

    fn find_segment(&self, id: &str) -> AppResult<Option<Segment>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .segments
            .iter()
            .find(|segment| segment.id == id)
            .cloned())
    }

    fn last_point(&self) -> AppResult<Option<PointRecord>> {
        Ok(self.inner.lock().unwrap().points.last().cloned())
    }

    fn history_segments(
        &self,
        from_ms: i64,
        to_ms: i64,
        _text: Option<&str>,
        _limit: u32,
        _offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)> {
        let items = self.day_segments(from_ms, to_ms)?;
        Ok((items.clone(), items.len() as u64))
    }

    fn categories(&self) -> AppResult<Vec<CategoryRow>> {
        Ok(self.inner.lock().unwrap().categories.clone())
    }

    fn settings(&self, scope: &str) -> AppResult<Vec<(String, String)>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .settings
            .iter()
            .filter(|(stored_scope, _, _)| stored_scope == scope)
            .map(|(_, key, value)| (key.clone(), value.clone()))
            .collect())
    }

    fn settings_upsert(
        &self,
        scope: &str,
        entries: &[(String, String)],
        _now_ms: i64,
    ) -> AppResult<()> {
        let mut data = self.inner.lock().unwrap();
        for (key, value) in entries {
            match data
                .settings
                .iter_mut()
                .find(|(stored_scope, stored_key, _)| stored_scope == scope && stored_key == key)
            {
                Some((_, _, stored)) => *stored = value.clone(),
                None => data
                    .settings
                    .push((scope.to_string(), key.clone(), value.clone())),
            }
        }
        Ok(())
    }

    fn settings_reset(&self, scope: &str, key: Option<&str>) -> AppResult<()> {
        let key = key.map(str::to_string);
        self.inner
            .lock()
            .unwrap()
            .settings
            .retain(|(stored_scope, stored_key, _)| {
                if stored_scope != scope {
                    return true;
                }
                match &key {
                    Some(key) => stored_key != key,
                    None => false,
                }
            });
        Ok(())
    }
}

#[derive(Clone)]
struct FixedClock {
    now: Arc<Mutex<Instant>>,
}

impl FixedClock {
    fn new(ms: i64) -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::new(ms, TZ))),
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }
}

fn service(store: FakeStore) -> TimerService<FakeStore, FixedClock> {
    TimerService::new(store, FixedClock::new(T0), PunchMode::Stop)
}

fn closed(id: &str, start: i64, end: i64, title: &str, category: Option<&str>) -> Segment {
    let mut segment = Segment::new_active(id.to_string(), Instant::new(start, TZ));
    segment.end_at_ms = Some(end);
    segment.title = title.to_string();
    segment.category_id = category.map(str::to_string);
    segment
}

#[test]
fn settings_merge_defaults_and_validate() {
    let store = FakeStore::default();
    let service = service(store);

    let defaults = service.settings(None).unwrap();
    assert_eq!(defaults["punch.mode"], "stop");
    assert_eq!(defaults["day.start_minutes"], 240);
    assert_eq!(defaults["focus.min_minutes"], 10);
    assert_eq!(defaults["ui.theme"], "system");

    let invalid = service
        .settings_patch(
            "app",
            vec![("punch.mode".into(), serde_json::json!("both"))],
        )
        .unwrap_err();
    assert_eq!(invalid.code(), "E_SETTING_INVALID");

    let unknown = service
        .settings_patch("app", vec![("nope.key".into(), serde_json::json!(1))])
        .unwrap_err();
    assert_eq!(unknown.code(), "E_SETTING_INVALID");

    let patched = service
        .settings_patch(
            "app",
            vec![
                ("punch.mode".into(), serde_json::json!("chain")),
                ("day.start_minutes".into(), serde_json::json!(300)),
            ],
        )
        .unwrap();
    assert_eq!(patched["punch.mode"], "chain");
    assert_eq!(patched["day.start_minutes"], 300);

    service.settings_reset("app", None).unwrap();
    let reset = service.settings(None).unwrap();
    assert_eq!(reset["punch.mode"], "stop");
}

#[test]
fn themes_round_trip_through_settings_and_reject_unknown_values() {
    let service = service(FakeStore::default());
    for theme in [
        "system", "light", "dark", "paper", "forest", "ocean", "lavender", "rose", "grid", "folio",
        "console", "glass",
    ] {
        service
            .settings_patch("app", vec![("ui.theme".into(), serde_json::json!(theme))])
            .unwrap();
        assert_eq!(service.settings(Some("app")).unwrap()["ui.theme"], theme);
    }
    assert_eq!(
        service
            .settings_patch(
                "app",
                vec![("ui.theme".into(), serde_json::json!("missing"))]
            )
            .unwrap_err()
            .code(),
        "E_SETTING_INVALID"
    );
    assert_eq!(service.settings(Some("app")).unwrap()["ui.theme"], "glass");
}

#[test]
fn stats_day_bucket_matches_segments() {
    let store = FakeStore::with_categories(&[("cat-work", "工作", "#0f766e")]);
    store.push_segment(closed(
        "seg-a",
        T0 - 3 * 3_600_000,
        T0 - 2 * 3_600_000,
        "写代码",
        Some("cat-work"),
    ));
    store.push_segment(closed(
        "seg-b",
        T0 - 2 * 3_600_000,
        T0 - 3_600_000 - 1_800_000,
        "",
        None,
    ));
    let service = service(store);

    let report = service
        .stats(T0 - 24 * 3_600_000, T0, Granularity::Day)
        .unwrap();
    assert_eq!(report.buckets.len(), 1);
    let bucket = &report.buckets[0];
    assert_eq!(bucket.recorded_seconds, 5_400);
    assert_eq!(bucket.effective_seconds, 3_600);
    assert_eq!(bucket.blank_seconds, 86_400 - 5_400);
    assert_eq!(bucket.segment_count, 2);
    assert!((bucket.coverage_rate - 3_600.0 / 86_400.0).abs() < 1e-9);
    assert_eq!(bucket.by_category.len(), 2);
    assert_eq!(
        bucket.by_category[0].category_id.as_deref(),
        Some("cat-work")
    );
    assert_eq!(bucket.by_category[0].seconds, 3_600);
}

#[test]
fn stats_month_granularity_returns_one_bucket() {
    let store = FakeStore::default();
    store.push_segment(closed("seg-a", T0 - 3_600_000, T0, "阅读", None));
    let service = service(store);

    let report = service
        .stats(T0 - 24 * 3_600_000, T0, Granularity::Month)
        .unwrap();
    assert_eq!(report.buckets.len(), 1);
    assert_eq!(report.buckets[0].recorded_seconds, 3_600);
}

#[test]
fn export_csv_then_import_round_trip() {
    let store = FakeStore::with_categories(&[("cat-study", "学习", "#2563eb")]);
    store.push_segment(closed(
        "seg-a",
        T0 - 3_600_000,
        T0,
        "高数, 第一章",
        Some("cat-study"),
    ));
    let service = service(store.clone());

    let payload = service
        .export(ExportFormat::Csv, T0 - 24 * 3_600_000, T0)
        .unwrap();
    assert_eq!(payload.count, 1);
    assert!(payload.content.starts_with('\u{feff}'));
    assert!(payload.content.contains("\"高数, 第一章\""));

    // 清空后从导出的 CSV 导入。
    store.inner.lock().unwrap().segments.clear();
    let preview = service.import_preview(&payload.content).unwrap();
    assert_eq!(preview.total, 1);
    assert_eq!(preview.importable, 1);
    assert_eq!(preview.rows[0].title, "高数, 第一章");
    assert_eq!(preview.rows[0].seconds, 3_600);

    let result = service.import_commit(&payload.content).unwrap();
    assert_eq!(result.inserted, 1);
    assert_eq!(result.skipped, 0);
    let segments = store.inner.lock().unwrap().segments.clone();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].title, "高数, 第一章");
    assert_eq!(segments[0].category_id.as_deref(), Some("cat-study"));
}

#[test]
fn import_reports_conflicts_and_invalid_rows() {
    let store = FakeStore::default();
    store.push_segment(closed("seg-existing", T0 - 3_600_000, T0, "已有", None));
    let service = service(store.clone());

    let csv = "date,start,end,duration_min,title,category,tags,note,is_focus,is_ignored,source\n\
               2026-10-26,13:00,15:00,120,重叠,,, ,0,0,manual\n\
               2026-10-26,20:00,21:00,60,正常,,, ,0,0,manual\n\
               2026-10-26,22:00,22:00,60,倒序,,, ,0,0,manual\n";
    let preview = service.import_preview(csv).unwrap();
    assert_eq!(preview.total, 3);
    assert_eq!(preview.conflicts, 1);
    assert_eq!(preview.invalid, 1);
    assert_eq!(preview.importable, 1);

    let result = service.import_commit(csv).unwrap();
    assert_eq!(result.inserted, 1);
    assert_eq!(result.skipped, 2);
}

#[test]
fn export_json_contains_schema_and_category_names() {
    let store = FakeStore::with_categories(&[("cat-life", "生活", "#b45309")]);
    store.push_segment(closed(
        "seg-a",
        T0 - 3_600_000,
        T0,
        "整理",
        Some("cat-life"),
    ));
    let service = service(store);

    let payload = service
        .export(ExportFormat::Json, T0 - 24 * 3_600_000, T0)
        .unwrap();
    let document: serde_json::Value = serde_json::from_str(&payload.content).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["data"]["segments"][0]["category"], "生活");
    assert_eq!(document["data"]["segments"][0]["title"], "整理");
    assert_eq!(document["app"]["name"], "dotlog");
}

#[test]
fn day_uses_start_minutes_from_settings() {
    let store = FakeStore::default();
    let service = service(store);
    let default_start = dotlog_domain::parse_day_window("2026-10-26", TZ, 240)
        .unwrap()
        .0;
    let midnight_start = dotlog_domain::parse_day_window("2026-10-26", TZ, 0)
        .unwrap()
        .0;

    let day = service.day(Some("2026-10-26")).unwrap();
    assert_eq!(day.window_start, default_start);

    service
        .settings_patch(
            "app",
            vec![("day.start_minutes".into(), serde_json::json!(0))],
        )
        .unwrap();
    let shifted = service.day(Some("2026-10-26")).unwrap();
    assert_eq!(shifted.window_start, midnight_start);
}

#[test]
fn backfill_stores_category_and_focus_flags() {
    let store = FakeStore::with_categories(&[("cat-fun", "娱乐", "#be185d")]);
    let service = service(store.clone());
    let created = service
        .backfill_segment(dotlog_application::dto::BackfillInput {
            start_at: T0 - 3_600_000,
            end_at: T0,
            title: Some("游戏".into()),
            note: None,
            category_id: Some("cat-fun".into()),
            is_focus: true,
            is_ignored: false,
        })
        .unwrap();
    assert_eq!(
        created.category.as_ref().map(|c| c.name.as_str()),
        Some("娱乐")
    );
    assert!(created.is_focus);
    assert!(!created.is_ignored);
}

#[test]
fn update_patch_sets_category_and_ignore_flag() {
    let store = FakeStore::with_categories(&[("cat-work", "工作", "#0f766e")]);
    store.push_segment(closed("seg-a", T0 - 3_600_000, T0, "写代码", None));
    let service = service(store);

    let updated = service
        .update_segment(
            "seg-a",
            1,
            dotlog_application::dto::SegmentPatch {
                category_id: Some(Some("cat-work".into())),
                is_ignored: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        updated.category.as_ref().map(|c| c.id.as_str()),
        Some("cat-work")
    );
    assert!(updated.is_ignored);

    // null 清空分类。
    let cleared = service
        .update_segment(
            "seg-a",
            updated.revision,
            dotlog_application::dto::SegmentPatch {
                category_id: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(cleared.category.is_none());
}

#[test]
fn motion_setting_accepts_only_known_preferences() {
    use dotlog_application::settings::validate_entry;
    for value in ["system", "reduced"] {
        assert!(validate_entry("app", "ui.motion", &serde_json::json!(value)).is_ok());
    }
    assert!(validate_entry("app", "ui.motion", &serde_json::json!("unknown")).is_err());
}
