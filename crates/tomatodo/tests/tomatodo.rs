//! 番茄Todo 插件测试：JSON 解析、有效记录筛选、时长计算、扫描预览。

use std::fs;
use std::path::PathBuf;

use dotlog_tomatodo::{
    compare, DotlogSegment, FocusRecord, ImportResult, TomatodoDb, default_db_path,
    resolve_db_path, scan,
};

/// 从真实数据精简的测试 fixture：4 条专注记录 + 6 条待办。
const FIXTURE: &str = r#"{
  "PCToDo": [
    {"id":1507,"name":"英语","originalName":"英语","time":-1,"isComplied":0,"repeatMode":0,"i1":0,"i2":0,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"1791686858484","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","type":0,"situation":0,"sourceType":"phone","phoneId":null,"isSynced":1},
    {"id":1508,"name":"政治","originalName":"政治","time":-1,"isComplied":0,"repeatMode":0,"i1":0,"i2":0,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"1791686858491","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","type":0,"situation":0,"sourceType":"phone","phoneId":null,"isSynced":1}
  ],
  "PCRecord": [
    {"id":1,"name":"高数","time":15,"startDate":1790757620243,"createDate":1790758572262,"i1":0,"i2":1,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","isComplete":1,"isSynced":1,"isTransfer":0,"boundDeviceId":null},
    {"id":2,"name":"线代","time":1,"startDate":1791424193751,"createDate":1791424299655,"i1":0,"i2":1,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","isComplete":1,"isSynced":0,"isTransfer":0,"boundDeviceId":null},
    {"id":3,"name":"高数","time":1,"startDate":1791424302581,"createDate":1791424404787,"i1":0,"i2":1,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","isComplete":0,"isSynced":0,"isTransfer":0,"boundDeviceId":null},
    {"id":4,"name":"高数","time":105,"startDate":1791442979035,"createDate":1791451761660,"i1":0,"i2":1,"i3":0,"i4":0,"i5":0,"i6":0,"i7":0,"i8":0,"i9":0,"s1":"","s2":"","s3":"","s4":"","s5":"","s6":"","s7":"","s8":"","s9":"","isComplete":1,"isSynced":0,"isTransfer":0,"boundDeviceId":null}
  ],
  "PCDeletedTodo": [],
  "todoIdCounter": 1512,
  "recordIdCounter": 4,
  "currentPhoneId": null,
  "isMidnightMode": false,
  "isMember": true,
  "phoneInstallTimes": {},
  "listPageHeight": 612,
  "trusted_devices": [],
  "last_active_device_id": null,
  "connectionHistory": []
}"#;

/// 返回 (临时目录句柄, 文件路径)；目录句柄必须被调用方持有，否则文件会被提前删除。
fn fixture_path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tomatodo_db.json");
    fs::write(&path, FIXTURE).unwrap();
    (dir, path)
}

// ---------- 解析 ----------

#[test]
fn parse_real_shape() {
    let db = TomatodoDb::from_json(FIXTURE).unwrap();
    assert_eq!(db.pc_record.len(), 4);
    assert_eq!(db.pc_todo.len(), 2);
    assert_eq!(db.record_id_counter, 4);
    assert_eq!(db.todo_id_counter, 1512);
}

#[test]
fn parse_handles_missing_optional_fields() {
    // 最小化结构：只有 PCRecord，无 PCToDo 等。
    let minimal = r#"{"PCRecord":[{"id":1,"name":"x","time":5,"startDate":1000,"createDate":2000,"isComplete":1}]}"#;
    let db = TomatodoDb::from_json(minimal).unwrap();
    assert_eq!(db.pc_record.len(), 1);
    assert!(db.pc_todo.is_empty());
    assert_eq!(db.record_id_counter, 0);
}

// ---------- 有效记录筛选 ----------

#[test]
fn only_completed_records_count() {
    let db = TomatodoDb::from_json(FIXTURE).unwrap();
    let completed = db.completed_records();
    // id=3 的 isComplete=0，应被过滤。
    assert_eq!(completed.len(), 3);
    let ids: Vec<i64> = completed.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![1, 2, 4]);
}

#[test]
fn zero_minute_record_excluded() {
    let json = r#"{"PCRecord":[{"id":9,"name":"x","time":0,"startDate":1,"createDate":2,"isComplete":1}]}"#;
    let db = TomatodoDb::from_json(json).unwrap();
    assert!(db.completed_records().is_empty());
}

// ---------- 时长计算 ----------

#[test]
fn end_time_uses_recorded_minutes() {
    let db = TomatodoDb::from_json(FIXTURE).unwrap();
    let record = &db.pc_record[0]; // 高数 15min
    assert_eq!(record.time, 15);
    assert_eq!(record.end_at_ms(), record.start_date + 15 * 60_000);
    // createDate 比 start+time 晚 52 秒（操作延迟），不应用作结束时间。
    assert!(record.create_date > record.end_at_ms());
}

#[test]
fn total_minutes_sum_completed() {
    let db = TomatodoDb::from_json(FIXTURE).unwrap();
    let total: i64 = db.completed_records().iter().map(|r| r.time).sum();
    // 15 + 1 + 105 = 121
    assert_eq!(total, 121);
}

// ---------- 扫描 ----------

#[test]
fn scan_returns_preview_sorted_desc() {
    let (_dir, path) = fixture_path();
    let result = scan(&path).unwrap();
    assert_eq!(result.total_records, 4);
    assert_eq!(result.completed_records, 3);
    assert_eq!(result.total_minutes, 121);
    assert_eq!(result.todo_count, 2);
    // 预览按 startDate 降序：id=4 (1791442979035) > id=2 (1791424193751) > id=1 (1790757620243)
    let preview_ids: Vec<i64> = result.preview.iter().map(|p| p.id).collect();
    assert_eq!(preview_ids, vec![4, 2, 1]);
}

#[test]
fn scan_preview_carries_correct_duration() {
    let (_dir, path) = fixture_path();
    let result = scan(&path).unwrap();
    let first = &result.preview[0]; // id=4, 高数 105min
    assert_eq!(first.name, "高数");
    assert_eq!(first.duration_minutes, 105);
    assert_eq!(first.end_at_ms, first.start_at_ms + 105 * 60_000);
}

#[test]
fn scan_missing_file_returns_error() {
    let missing = PathBuf::from("nonexistent_tomatodo_db.json");
    let result = scan(&missing);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("无法读取"));
}

// ---------- 路径解析 ----------

#[test]
fn empty_path_falls_back_to_default() {
    let resolved = resolve_db_path(None);
    assert!(resolved.ends_with("tomatodo_db.json"));
    assert!(resolved.starts_with(default_db_path().parent().unwrap()));
}

#[test]
fn custom_path_takes_precedence() {
    let resolved = resolve_db_path(Some("D:\\data\\tomatodo.json"));
    assert_eq!(resolved, PathBuf::from("D:\\data\\tomatodo.json"));
}

// ---------- 导入结果默认值 ----------

#[test]
fn import_result_default_is_empty() {
    let result = ImportResult::default();
    assert_eq!(result.imported, 0);
    assert_eq!(result.skipped_existing, 0);
    assert_eq!(result.skipped_incomplete, 0);
    assert_eq!(result.conflicts, 0);
    assert!(result.errors.is_empty());
}

// ---------- 预览结构序列化 ----------

#[test]
fn scan_result_serializes_to_camel_case() {
    let (_dir, path) = fixture_path();
    let result = scan(&path).unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("dbPath").is_some());
    assert!(json.get("fileMtimeMs").is_some());
    assert!(json.get("completedRecords").is_some());
    assert!(json.get("totalMinutes").is_some());
    assert!(json.get("earliestStartMs").is_some());
    let preview = json.get("preview").and_then(|v| v.as_array()).unwrap();
    assert!(preview[0].get("startAtMs").is_some());
    assert!(preview[0].get("durationMinutes").is_some());
}

// ---------- 对比 ----------

fn make_record(id: i64, name: &str, minutes: i64, start: i64) -> FocusRecord {
    FocusRecord {
        id,
        name: name.into(),
        time: minutes,
        start_date: start,
        create_date: start + minutes * 60_000,
        is_complete: 1,
        is_synced: 0,
        is_transfer: 0,
        bound_device_id: None,
    }
}

fn make_segment(id: &str, title: &str, start: i64, end: i64) -> DotlogSegment {
    DotlogSegment {
        id: id.into(),
        start_at_ms: start,
        end_at_ms: end,
        title: title.into(),
        category_name: None,
        tags: Vec::new(),
    }
}

#[test]
fn compare_matches_overlapping_segments() {
    // 番茄Todo：09:00-09:25 高数（25分钟）
    let records = vec![make_record(1, "高数", 25, 9 * 3_600_000)];
    // DotLog：09:00-09:15 做高数题，09:15-09:30 休息
    let segments = vec![
        make_segment("s1", "做高数 880 题", 9 * 3_600_000, 9 * 3_600_000 + 15 * 60_000),
        make_segment("s2", "休息", 9 * 3_600_000 + 15 * 60_000, 9 * 3_600_000 + 30 * 60_000),
    ];
    let result = compare(&records, &segments);
    assert_eq!(result.summary.matched_pairs, 1);
    assert_eq!(result.summary.missing_in_dotlog, 0);
    assert_eq!(result.summary.extra_in_dotlog, 0);
    // 番茄Todo 25分钟，DotLog 覆盖 15+15=30 分钟（含休息），覆盖 100%
    assert_eq!(result.pairs[0].coverage_percent, 100);
    assert_eq!(result.pairs[0].dotlog_segments.len(), 2);
}

#[test]
fn compare_detects_missing_in_dotlog() {
    // 番茄Todo 有记录，但 DotLog 完全没有对应段
    let records = vec![make_record(1, "高数", 25, 9 * 3_600_000)];
    let segments: Vec<DotlogSegment> = Vec::new();
    let result = compare(&records, &segments);
    assert_eq!(result.summary.missing_in_dotlog, 1);
    assert_eq!(result.summary.matched_pairs, 0);
    assert!(result.pairs[0].dotlog_segments.is_empty());
}

#[test]
fn compare_detects_extra_in_dotlog() {
    // DotLog 有段，但番茄Todo 没有对应记录
    let records: Vec<FocusRecord> = Vec::new();
    let segments = vec![make_segment("s1", "自由学习", 9 * 3_600_000, 9 * 3_600_000 + 15 * 60_000)];
    let result = compare(&records, &segments);
    assert_eq!(result.summary.extra_in_dotlog, 1);
    assert!(result.pairs[0].tomatodo.is_none());
}

#[test]
fn compare_skips_incomplete_records() {
    // isComplete=0 的记录不参与对比
    let mut record = make_record(1, "高数", 25, 9 * 3_600_000);
    record.is_complete = 0;
    let records = vec![record];
    let segments = vec![make_segment("s1", "x", 9 * 3_600_000, 9 * 3_600_000 + 10 * 60_000)];
    let result = compare(&records, &segments);
    assert_eq!(result.summary.tomatodo_minutes, 0);
    // 未完成的番茄Todo 记录被忽略，DotLog 段变成 extra
    assert_eq!(result.summary.extra_in_dotlog, 1);
}

#[test]
fn compare_partial_coverage() {
    // 番茄Todo：09:00-09:25（25分钟）
    // DotLog：09:00-09:10（10分钟）
    let records = vec![make_record(1, "高数", 25, 9 * 3_600_000)];
    let segments = vec![make_segment("s1", "高数", 9 * 3_600_000, 9 * 3_600_000 + 10 * 60_000)];
    let result = compare(&records, &segments);
    // 覆盖 10/25 = 40%
    assert_eq!(result.pairs[0].coverage_percent, 40);
}

#[test]
fn compare_coverage_clamped_to_100() {
    // 多个 DotLog 段同时覆盖同一条番茄Todo 记录时，overlap 可能超过番茄Todo 时长，
    // 覆盖率必须 clamp 到 100，不能因 u8 溢出回绕。
    let records = vec![make_record(1, "高数", 10, 9 * 3_600_000)];
    let segments = vec![
        make_segment("s1", "做题", 9 * 3_600_000, 9 * 3_600_000 + 10 * 60_000),
        make_segment("s2", "做题", 9 * 3_600_000, 9 * 3_600_000 + 10 * 60_000),
    ];
    let result = compare(&records, &segments);
    assert_eq!(result.pairs[0].coverage_percent, 100);
}

