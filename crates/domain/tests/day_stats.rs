//! 日窗口、跨日切片、空白与统计口径（`docs/domain-model.md` §6 / §10）。

use dotlog_domain::{clip, day_window, gaps, local_date, parse_day_window, totals, Clip, Segment};

const TZ: i32 = 480;
const DAY_START: i32 = 240;

fn segment(id: &str, start: i64, end: Option<i64>, title: &str) -> Segment {
    Segment {
        id: id.to_string(),
        start_at_ms: start,
        end_at_ms: end,
        title: title.to_string(),
        note: String::new(),
        tags: vec![],
        category_id: None,
        is_focus: false,
        is_ignored: false,
        tz_offset_min: TZ,
        revision: 1,
    }
}

#[test]
fn day_window_starts_at_four_local() {
    // 2023-11-15 06:13:20 +08:00 属于 11-15 的 04:00 起窗口。
    let (start, end) = day_window(1_700_000_000_000, TZ, DAY_START);
    assert_eq!(start, 1_699_992_000_000); // 2023-11-14T20:00:00Z
    assert_eq!(end - start, 86_400_000);
    assert_eq!(local_date(1_700_000_000_000, TZ, DAY_START), "2023-11-15");
}

#[test]
fn early_morning_belongs_to_previous_day() {
    // 2023-11-15 03:30 +08:00 仍在 11-14 的日窗口内。
    let at = 1_699_992_000_000 - 30 * 60_000;
    assert_eq!(local_date(at, TZ, DAY_START), "2023-11-14");
    let (start, _) = day_window(at, TZ, DAY_START);
    assert_eq!(start, 1_699_992_000_000 - 86_400_000);
}

#[test]
fn parse_day_window_rejects_invalid_dates() {
    assert!(parse_day_window("2023-11-15", TZ, DAY_START).is_some());
    assert!(parse_day_window("2023-02-31", TZ, DAY_START).is_none());
    assert!(parse_day_window("2023-13-01", TZ, DAY_START).is_none());
    assert!(parse_day_window("2023/11/15", TZ, DAY_START).is_none());
}

#[test]
fn parse_and_local_date_round_trip() {
    let (start, _) = parse_day_window("2026-10-28", TZ, DAY_START).expect("合法日期");
    assert_eq!(local_date(start, TZ, DAY_START), "2026-10-28");
    assert_eq!(local_date(start - 1, TZ, DAY_START), "2026-10-27");
}

#[test]
fn clip_handles_cross_midnight_segment() {
    let window_start = 1_000;
    let window_end = 5_000;
    let seg = segment("s1", 0, Some(10_000), "");
    let clipped = clip(&seg, window_start, window_end, 10_000).expect("应落在窗口内");
    assert_eq!(clipped, Clip { start_ms: 1_000, end_ms: 5_000 });

    let outside = segment("s2", 6_000, Some(9_000), "");
    assert!(clip(&outside, window_start, window_end, 10_000).is_none());
}

#[test]
fn active_segment_clips_at_now() {
    let seg = segment("s1", 1_000, None, "");
    let clipped = clip(&seg, 0, 86_400_000, 7_000).expect("active 段应可见");
    assert_eq!(clipped, Clip { start_ms: 1_000, end_ms: 7_000 });
}

#[test]
fn gaps_fill_window_between_segments() {
    let clips = [
        Clip { start_ms: 100, end_ms: 200 },
        Clip { start_ms: 300, end_ms: 400 },
    ];
    let result = gaps(&clips, 0, 500);
    assert_eq!(
        result,
        vec![
            Clip { start_ms: 0, end_ms: 100 },
            Clip { start_ms: 200, end_ms: 300 },
            Clip { start_ms: 400, end_ms: 500 },
        ]
    );
}

#[test]
fn gaps_ignore_touching_segments() {
    let clips = [
        Clip { start_ms: 100, end_ms: 200 },
        Clip { start_ms: 200, end_ms: 300 },
    ];
    let result = gaps(&clips, 100, 300);
    assert!(result.is_empty());
}

#[test]
fn totals_count_recorded_effective_and_blank() {
    let day_start = 0;
    let now = 86_400_000;
    let segments = [
        segment("s1", 0, Some(3_600_000), "写作"),
        segment("s2", 3_600_000, Some(7_200_000), ""),
    ];
    let (totals, coverage) = totals(&segments, day_start, now, now);
    assert_eq!(totals.recorded_seconds, 7_200);
    assert_eq!(totals.effective_seconds, 3_600);
    assert_eq!(totals.blank_seconds, 86_400 - 7_200);
    assert_eq!(totals.segment_count, 2);
    assert_eq!(totals.average_segment_seconds, 3_600);
    assert_eq!(totals.switch_count, 0);
    assert!((coverage - 3_600.0 / 86_400.0).abs() < 1e-9);
}

#[test]
fn totals_cap_window_at_now_for_today() {
    let now = 10 * 3_600_000;
    let segments = [segment("s1", 0, Some(2 * 3_600_000), "阅读")];
    let (totals, _) = totals(&segments, 0, now, now);
    assert_eq!(totals.blank_seconds, 8 * 3_600);
    assert_eq!(totals.recorded_seconds, 7_200);
}
