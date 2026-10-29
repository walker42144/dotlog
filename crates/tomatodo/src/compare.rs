//! 番茄Todo 与 DotLog 数据对比（`docs/plugin-sdk.md` §9.2）。
//!
//! 番茄Todo 只记录「科目 + 起止时间」（粗粒度），DotLog 记录「具体活动 + 起止时间」
//! （细粒度）。本模块按时间重叠把两者配对，找出：
//! - 番茄Todo 有但 DotLog 没有的时间段（漏记）
//! - DotLog 有但番茄Todo 没有的时间段（番茄Todo 漏记或自由活动）
//! - 同一时间段两者的内容差异（科目 vs 具体活动）

use serde::{Deserialize, Serialize};

use crate::reader::{FocusRecord, FocusPreview};

/// DotLog 段的最小视图（由桌面端从 store 查询后传入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DotlogSegment {
    pub id: String,
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub title: String,
    #[serde(default)]
    pub category_name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 对比结果中的单条配对。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparePair {
    /// 番茄Todo 记录（可能为 None，表示 DotLog 有但番茄Todo 没有）。
    pub tomatodo: Option<FocusPreview>,
    /// 与之时间重叠的 DotLog 段列表。
    pub dotlog_segments: Vec<DotlogSegment>,
    /// 重叠秒数。
    pub overlap_seconds: i64,
    /// 番茄Todo 时长（秒）。
    pub tomatodo_seconds: i64,
    /// DotLog 总时长（秒，可能多个段）。
    pub dotlog_seconds: i64,
    /// 覆盖比例：DotLog 覆盖番茄Todo 的百分比（0-100）。
    pub coverage_percent: u8,
}

/// 对比统计。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompareSummary {
    /// 番茄Todo 专注总分钟数。
    pub tomatodo_minutes: i64,
    /// DotLog 段总分钟数（在番茄Todo 时间范围内）。
    pub dotlog_minutes: i64,
    /// 番茄Todo 有但 DotLog 完全没有的记录数。
    pub missing_in_dotlog: usize,
    /// DotLog 有但番茄Todo 没有的段数。
    pub extra_in_dotlog: usize,
    /// 有重叠的配对数。
    pub matched_pairs: usize,
}

/// 对比结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareResult {
    pub summary: CompareSummary,
    pub pairs: Vec<ComparePair>,
}

/// 计算两个时间段的重叠秒数（无重叠返回 0）。
fn overlap_seconds(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> i64 {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    if end > start {
        (end - start) / 1000
    } else {
        0
    }
}

/// 把番茄Todo 记录与 DotLog 段按时间重叠配对。
///
/// 规则：
/// - 每条番茄Todo 记录找出所有与之有时间重叠的 DotLog 段；
/// - DotLog 段如果不与任何番茄Todo 记录重叠，归为「extra_in_dotlog」。
pub fn compare(records: &[FocusRecord], segments: &[DotlogSegment]) -> CompareResult {
    let mut pairs: Vec<ComparePair> = Vec::new();
    let mut matched_segment_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for record in records {
        if !record.is_completed() {
            continue;
        }
        let rec_start = record.start_date;
        let rec_end = record.end_at_ms();

        let mut overlapping: Vec<&DotlogSegment> = segments
            .iter()
            .filter(|seg| overlap_seconds(rec_start, rec_end, seg.start_at_ms, seg.end_at_ms) > 0)
            .collect();
        overlapping.sort_by_key(|seg| seg.start_at_ms);

        let overlap_secs: i64 = overlapping
            .iter()
            .map(|seg| overlap_seconds(rec_start, rec_end, seg.start_at_ms, seg.end_at_ms))
            .sum();

        for seg in &overlapping {
            matched_segment_ids.insert(seg.id.clone());
        }

        let dotlog_secs: i64 = overlapping
            .iter()
            .map(|seg| (seg.end_at_ms - seg.start_at_ms) / 1000)
            .sum();

        let tomatodo_secs = (rec_end - rec_start) / 1000;
        // 多个 DotLog 段可能同时覆盖同一条番茄Todo 记录，导致 overlap > 番茄Todo 时长；
        // 先 clamp 到 [0, 100] 再转 u8，避免 f64→u8 溢出回绕。
        let coverage = if tomatodo_secs > 0 {
            ((overlap_secs as f64 / tomatodo_secs as f64) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8
        } else {
            0
        };

        pairs.push(ComparePair {
            tomatodo: Some(FocusPreview {
                id: record.id,
                name: record.name.clone(),
                start_at_ms: rec_start,
                end_at_ms: rec_end,
                duration_minutes: record.time,
                start_text: crate::reader::format_ms(rec_start),
                end_text: crate::reader::format_ms(rec_end),
            }),
            dotlog_segments: overlapping.into_iter().cloned().collect(),
            overlap_seconds: overlap_secs,
            tomatodo_seconds: tomatodo_secs,
            dotlog_seconds: dotlog_secs,
            coverage_percent: coverage,
        });
    }

    // DotLog 有但番茄Todo 没有的段。
    let extra: Vec<DotlogSegment> = segments
        .iter()
        .filter(|seg| !matched_segment_ids.contains(&seg.id))
        .cloned()
        .collect();

    for seg in &extra {
        let secs = (seg.end_at_ms - seg.start_at_ms) / 1000;
        pairs.push(ComparePair {
            tomatodo: None,
            dotlog_segments: vec![seg.clone()],
            overlap_seconds: 0,
            tomatodo_seconds: 0,
            dotlog_seconds: secs,
            coverage_percent: 0,
        });
    }

    let tomatodo_minutes: i64 = records
        .iter()
        .filter(|r| r.is_completed())
        .map(|r| r.time)
        .sum();
    let dotlog_minutes: i64 = segments
        .iter()
        .map(|s| (s.end_at_ms - s.start_at_ms) / 60_000)
        .sum();

    let missing_in_dotlog = pairs
        .iter()
        .filter(|p| p.tomatodo.is_some() && p.dotlog_segments.is_empty())
        .count();
    let matched_pairs = pairs
        .iter()
        .filter(|p| p.tomatodo.is_some() && !p.dotlog_segments.is_empty())
        .count();

    CompareResult {
        summary: CompareSummary {
            tomatodo_minutes,
            dotlog_minutes,
            missing_in_dotlog,
            extra_in_dotlog: extra.len(),
            matched_pairs,
        },
        pairs,
    }
}
