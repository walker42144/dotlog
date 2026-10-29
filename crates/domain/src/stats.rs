//! 统计口径（`docs/domain-model.md` §10）：日 / 周 / 月桶、分类占比、专注与完整率。
//!
//! 唯一实现：UI 只渲染本模块的计算结果，不自行统计。

use serde::{Deserialize, Serialize};

use crate::day::{clip, local_date, parse_day_window, DayPolicy};
use crate::model::Segment;

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_MINUTE: i64 = 60_000;

/// 统计粒度（`stats_get` 的 `granularity`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Day,
    Week,
    Month,
}

/// 分类时长占比的一项；`category_id = None` 表示未分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategorySeconds {
    pub category_id: Option<String>,
    pub seconds: i64,
}

/// 单个统计桶的结果（日 / 周 / 月共用同一口径）。
#[derive(Debug, Clone, PartialEq)]
pub struct BucketStats {
    pub recorded_seconds: i64,
    pub effective_seconds: i64,
    pub focus_seconds: i64,
    pub blank_seconds: i64,
    pub coverage_rate: f64,
    pub switch_count: i64,
    pub segment_count: i64,
    pub average_segment_seconds: i64,
    /// 按秒数降序；忽略段不计入。
    pub by_category: Vec<CategorySeconds>,
}

/// 计算 `[window_start, window_end)` 内的统计；`now_ms` 用于 active 段与未来区间截断。
pub fn bucket(
    segments: &[Segment],
    window_start: i64,
    window_end: i64,
    now_ms: i64,
    policy: DayPolicy,
) -> BucketStats {
    let focus_threshold_ms = policy.focus_min_minutes.max(1) * MS_PER_MINUTE;
    let mut recorded_ms = 0;
    let mut effective_ms = 0;
    let mut focus_ms = 0;
    let mut segment_count = 0;
    let mut effective_count = 0;
    let mut categories: Vec<(Option<String>, i64)> = Vec::new();

    for segment in segments {
        let Some(slice) = clip(segment, window_start, window_end, now_ms) else {
            continue;
        };
        let sliced_ms = slice.end_ms - slice.start_ms;
        segment_count += 1;
        recorded_ms += sliced_ms;

        let effective = !segment.is_ignored
            && (!segment.title.is_empty() || segment.category_id.is_some());
        if effective {
            effective_ms += sliced_ms;
            effective_count += 1;
        }
        if segment.is_focus && segment.duration_ms(now_ms) >= focus_threshold_ms {
            focus_ms += sliced_ms;
        }
        if !segment.is_ignored {
            match categories
                .iter_mut()
                .find(|(category, _)| *category == segment.category_id)
            {
                Some((_, seconds)) => *seconds += sliced_ms,
                None => categories.push((segment.category_id.clone(), sliced_ms)),
            }
        }
    }

    let window_ms = (window_end - window_start).max(0);
    let blank_ms = (window_ms - recorded_ms).max(0);
    let coverage_rate = if window_ms > 0 {
        (effective_ms as f64 / window_ms as f64).min(1.0)
    } else {
        0.0
    };

    let mut by_category: Vec<CategorySeconds> = categories
        .into_iter()
        .map(|(category_id, ms)| CategorySeconds {
            category_id,
            seconds: ms / 1_000,
        })
        .filter(|item| item.seconds > 0)
        .collect();
    by_category.sort_by_key(|item| std::cmp::Reverse(item.seconds));

    BucketStats {
        recorded_seconds: recorded_ms / 1_000,
        effective_seconds: effective_ms / 1_000,
        focus_seconds: focus_ms / 1_000,
        blank_seconds: blank_ms / 1_000,
        coverage_rate,
        switch_count: (effective_count - 1).max(0),
        segment_count,
        average_segment_seconds: if segment_count > 0 {
            recorded_ms / 1_000 / segment_count
        } else {
            0
        },
        by_category,
    }
}

/// 按粒度切分 `[from_ms, to_ms)`：日 / 周为 7 天步进，月按本地日历月对齐。
///
/// 所有边界都基于本地日边界（`day_start_minutes`），因此周不会跨逻辑日。
pub fn bucket_windows(
    from_ms: i64,
    to_ms: i64,
    granularity: Granularity,
    tz_offset_min: i32,
    day_start_minutes: i32,
) -> Vec<(i64, i64)> {
    if to_ms <= from_ms {
        return Vec::new();
    }

    let mut windows = Vec::new();
    let mut cursor = from_ms;
    while cursor < to_ms {
        let next = match granularity {
            Granularity::Day => cursor + MS_PER_DAY,
            Granularity::Week => cursor + 7 * MS_PER_DAY,
            Granularity::Month => {
                let date = local_date(cursor, tz_offset_min, day_start_minutes);
                match next_month_first_day(&date)
                    .and_then(|key| parse_day_window(&key, tz_offset_min, day_start_minutes))
                {
                    Some((start, _)) => start,
                    None => break,
                }
            }
        };
        let end = next.min(to_ms);
        if end <= cursor {
            break;
        }
        windows.push((cursor, end));
        cursor = end;
    }
    windows
}

/// `2026-10-29` → 下个月 1 日 `2026-11-01`。
fn next_month_first_day(date: &str) -> Option<String> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || parts.next().is_none() {
        return None;
    }
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Some(format!("{year:04}-{month:02}-01"))
}
