//! 日窗口、日期归属、跨日切片、空白与统计口径（`docs/domain-model.md` §6 / §10）。
//!
//! 纯整数运算：不依赖时区数据库，使用调用方传入的记录时区偏移。

use crate::model::Segment;

pub const DEFAULT_DAY_START_MINUTES: i32 = 240;
pub const DEFAULT_FOCUS_MIN_MINUTES: i64 = 10;
const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_MINUTE: i64 = 60_000;

/// 日视图口径。`day_start_minutes` 默认 04:00。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayPolicy {
    pub day_start_minutes: i32,
    pub focus_min_minutes: i64,
}

impl Default for DayPolicy {
    fn default() -> Self {
        Self {
            day_start_minutes: DEFAULT_DAY_START_MINUTES,
            focus_min_minutes: DEFAULT_FOCUS_MIN_MINUTES,
        }
    }
}

/// 某时刻所属的本地日期 `YYYY-MM-DD`。
pub fn local_date(utc_ms: i64, tz_offset_min: i32, day_start_minutes: i32) -> String {
    let (year, month, day) = civil_from_days(day_index(utc_ms, tz_offset_min, day_start_minutes));
    format!("{year:04}-{month:02}-{day:02}")
}

/// 某时刻所属本地日的窗口 `[start, start + 24h)`（UTC 毫秒）。
pub fn day_window(utc_ms: i64, tz_offset_min: i32, day_start_minutes: i32) -> (i64, i64) {
    let start = day_index(utc_ms, tz_offset_min, day_start_minutes) * MS_PER_DAY
        + minutes_ms(day_start_minutes)
        - minutes_ms(tz_offset_min);
    (start, start + MS_PER_DAY)
}

/// 解析 `YYYY-MM-DD` 为该时区下的日窗口；非法日期返回 `None`。
pub fn parse_day_window(
    date: &str,
    tz_offset_min: i32,
    day_start_minutes: i32,
) -> Option<(i64, i64)> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    // 拒绝 2 月 31 日这类溢出日期。
    if civil_from_days(days) != (year, month, day) {
        return None;
    }

    let start = days * MS_PER_DAY + minutes_ms(day_start_minutes) - minutes_ms(tz_offset_min);
    Some((start, start + MS_PER_DAY))
}

/// 段在窗口内的切片；完全落在窗口外返回 `None`。
pub fn clip(segment: &Segment, window_start: i64, window_end: i64, now_ms: i64) -> Option<Clip> {
    let end = segment.end_at_ms.unwrap_or(now_ms);
    let start = segment.start_at_ms.max(window_start);
    let end = end.min(window_end);
    (end > start).then_some(Clip { start_ms: start, end_ms: end })
}

/// 窗口内的空档，按时间升序；首尾相接的切片不会产生 0 长空档。
pub fn gaps(clips: &[Clip], window_start: i64, window_end: i64) -> Vec<Clip> {
    let mut ordered: Vec<Clip> = clips
        .iter()
        .copied()
        .filter(|clip| clip.end_ms > clip.start_ms)
        .collect();
    ordered.sort_by_key(|clip| clip.start_ms);

    let mut result = Vec::new();
    let mut cursor = window_start;
    for clip in ordered {
        if clip.start_ms > cursor {
            result.push(Clip {
                start_ms: cursor,
                end_ms: clip.start_ms.min(window_end),
            });
        }
        cursor = cursor.max(clip.end_ms);
        if cursor >= window_end {
            break;
        }
    }
    if cursor < window_end {
        result.push(Clip {
            start_ms: cursor,
            end_ms: window_end,
        });
    }
    result
}

/// 今日统计口径。窗口末端为 `min(日边界, now)`，不把未来算成空白。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayTotals {
    pub recorded_seconds: i64,
    pub effective_seconds: i64,
    pub blank_seconds: i64,
    pub focus_seconds: i64,
    pub switch_count: i64,
    pub segment_count: i64,
    pub average_segment_seconds: i64,
}

pub fn totals(
    segments: &[Segment],
    window_start: i64,
    window_end: i64,
    now_ms: i64,
) -> (DayTotals, f64) {
    let stats = crate::stats::bucket(
        segments,
        window_start,
        window_end,
        now_ms,
        DayPolicy::default(),
    );
    (
        DayTotals {
            recorded_seconds: stats.recorded_seconds,
            effective_seconds: stats.effective_seconds,
            blank_seconds: stats.blank_seconds,
            focus_seconds: stats.focus_seconds,
            switch_count: stats.switch_count,
            segment_count: stats.segment_count,
            average_segment_seconds: stats.average_segment_seconds,
        },
        stats.coverage_rate,
    )
}

/// 窗口内的段切片（按开始时间升序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Clip {
    pub fn seconds(self) -> i64 {
        (self.end_ms - self.start_ms) / 1_000
    }
}

fn minutes_ms(minutes: i32) -> i64 {
    minutes as i64 * MS_PER_MINUTE
}

fn day_index(utc_ms: i64, tz_offset_min: i32, day_start_minutes: i32) -> i64 {
    (utc_ms + minutes_ms(tz_offset_min) - minutes_ms(day_start_minutes)).div_euclid(MS_PER_DAY)
}

/// Howard Hinnant 的民用历算法：`1970-01-01` 起的天数 → (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 }.div_euclid(400);
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
