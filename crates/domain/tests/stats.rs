//! 统计口径测试（`docs/domain-model.md` §10）：分类、专注阈值、忽略与桶切分。

use dotlog_domain::{
    bucket_windows, local_date, stats_bucket, CategorySeconds, DayPolicy, Granularity, Segment,
};

const TZ: i32 = 480;
const DAY_START: i32 = 240;
const HOUR: i64 = 3_600_000;
const MINUTE: i64 = 60_000;
/// 2026-10-28 06:00 +08:00（落在 10-28 逻辑日窗口内）。
const BASE: i64 = 1_793_000_000_000;

fn segment(id: &str, start: i64, end: i64, title: &str) -> Segment {
    Segment {
        id: id.to_string(),
        start_at_ms: start,
        end_at_ms: Some(end),
        title: title.to_string(),
        note: String::new(),
        tags: Vec::new(),
        category_id: None,
        is_focus: false,
        is_ignored: false,
        tz_offset_min: TZ,
        revision: 1,
    }
}

fn policy() -> DayPolicy {
    DayPolicy::default()
}

#[test]
fn ignored_segment_is_recorded_but_not_effective() {
    let mut ignored = segment("a", BASE, BASE + HOUR, "休息");
    ignored.is_ignored = true;
    let stats = stats_bucket(&[ignored], BASE, BASE + 2 * HOUR, BASE + 2 * HOUR, policy());

    assert_eq!(stats.recorded_seconds, 3_600);
    assert_eq!(stats.effective_seconds, 0);
    assert_eq!(stats.coverage_rate, 0.0);
    assert!(stats.by_category.is_empty());
}

#[test]
fn focus_threshold_judges_whole_segment_duration() {
    let short = segment("short", BASE, BASE + 10 * MINUTE - 1_000, "短");
    let long = segment("long", BASE + HOUR, BASE + HOUR + 10 * MINUTE, "长");
    let mut short = short;
    short.is_focus = true;
    let mut long = long;
    long.is_focus = true;

    let stats = stats_bucket(
        &[short, long],
        BASE,
        BASE + 2 * HOUR,
        BASE + 2 * HOUR,
        policy(),
    );
    assert_eq!(stats.focus_seconds, 600);
}

#[test]
fn category_seconds_sorted_desc_and_uncategorized_kept() {
    let mut reading = segment("r", BASE, BASE + HOUR, "读书");
    reading.category_id = Some("cat-reading".to_string());
    let work = segment("w", BASE + HOUR, BASE + HOUR + 30 * MINUTE, "写代码");
    let mut work = work;
    work.category_id = Some("cat-work".to_string());

    let stats = stats_bucket(
        &[reading, work],
        BASE,
        BASE + 3 * HOUR,
        BASE + 3 * HOUR,
        policy(),
    );

    assert_eq!(
        stats.by_category,
        vec![
            CategorySeconds {
                category_id: Some("cat-reading".to_string()),
                seconds: 3_600
            },
            CategorySeconds {
                category_id: Some("cat-work".to_string()),
                seconds: 1_800
            },
        ]
    );
}

#[test]
fn switch_count_and_coverage_use_effective_segments() {
    let first = segment("a", BASE, BASE + HOUR, "一");
    let second = segment("b", BASE + HOUR, BASE + 2 * HOUR, "二");
    let stats = stats_bucket(
        &[first, second],
        BASE,
        BASE + 4 * HOUR,
        BASE + 4 * HOUR,
        policy(),
    );

    assert_eq!(stats.switch_count, 1);
    assert_eq!(stats.coverage_rate, 0.5);
    assert_eq!(stats.average_segment_seconds, 3_600);
}

#[test]
fn bucket_windows_split_by_day_and_month() {
    let (day_start, _day_end) = dotlog_domain::day_window(BASE, TZ, DAY_START);
    let days = bucket_windows(day_start, day_start + 3 * 86_400_000, Granularity::Day, TZ, DAY_START);
    assert_eq!(days.len(), 3);
    assert_eq!(days[0].0, day_start);
    assert_eq!(days[0].1, day_start + 86_400_000);

    let weeks = bucket_windows(day_start, day_start + 14 * 86_400_000, Granularity::Week, TZ, DAY_START);
    assert_eq!(weeks.len(), 2);

    let months = bucket_windows(day_start, day_start + 70 * 86_400_000, Granularity::Month, TZ, DAY_START);
    assert!(months.len() >= 2);
    let first_label = local_date(months[0].0, TZ, DAY_START);
    let second_label = local_date(months[1].0, TZ, DAY_START);
    assert_ne!(first_label[0..7], second_label[0..7]);
    assert!(second_label.ends_with("-01"));
}
