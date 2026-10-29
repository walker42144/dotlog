//! CSV 导入：解析、预览与冲突报告（`docs/data-dictionary.md` §7.2）。
//!
//! 预览与提交共用同一解析逻辑；提交只写入 `ok` 行，冲突与非法行跳过并计数。

use std::collections::HashMap;

use dotlog_domain::{Effect, Segment};

use crate::dto::{ImportPreviewView, ImportResultView, ImportRowView};
use crate::error::{AppError, AppResult};
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;
use crate::timefmt::{parse_date_time, parse_iso_local};

const MS_PER_DAY: i64 = 86_400_000;
/// 单行最长 24 小时，与补录上限一致。
const MAX_ROW_MS: i64 = MS_PER_DAY;

struct ParsedRow {
    index: usize,
    start_at: i64,
    end_at: i64,
    title: String,
    note: String,
    category_name: Option<String>,
    is_focus: bool,
    is_ignored: bool,
    status: &'static str,
}

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// `import_preview`：逐行解析并标注 `ok` / `conflict` / `invalid`。
    pub fn import_preview(&self, content: &str) -> AppResult<ImportPreviewView> {
        let analysis = self.analyze_import(content)?;
        Ok(analysis.preview)
    }

    /// `import_commit`：写入 `ok` 行；冲突与非法行跳过。
    pub fn import_commit(&self, content: &str) -> AppResult<ImportResultView> {
        let analysis = self.analyze_import(content)?;
        let tz = self.clock.now().tz_offset_min;
        let now_ms = self.clock.now().utc_ms;
        let names = self.category_ids()?;

        let mut effects = Vec::new();
        for row in &analysis.rows {
            if row.status != "ok" {
                continue;
            }
            let category_id = row
                .category_name
                .as_ref()
                .and_then(|name| names.get(name))
                .cloned();
            effects.push(Effect::InsertSegment(Segment {
                id: crate::new_id(),
                start_at_ms: row.start_at,
                end_at_ms: Some(row.end_at),
                title: row.title.clone(),
                note: row.note.clone(),
                tags: Vec::new(),
                category_id,
                is_focus: row.is_focus,
                is_ignored: row.is_ignored,
                tz_offset_min: tz,
                revision: 1,
            }));
        }

        let inserted = effects.len() as u64;
        if !effects.is_empty() {
            self.store.commit(&effects, now_ms)?;
        }
        Ok(ImportResultView {
            inserted,
            skipped: analysis.preview.total - inserted,
        })
    }

    fn analyze_import(&self, content: &str) -> AppResult<ImportAnalysis> {
        let tz = self.clock.now().tz_offset_min;
        let records = parse_records(content);
        let Some(header) = records.first() else {
            return Err(AppError::store("E_INVALID_RANGE", "CSV 内容为空"));
        };
        let columns = HeaderColumns::new(header)?;

        let mut rows = Vec::new();
        let mut invalid = Vec::new();
        for (offset, record) in records.iter().enumerate().skip(1) {
            if record.iter().all(|field| field.trim().is_empty()) {
                continue;
            }
            match columns.parse(record, offset, tz) {
                Ok(row) => rows.push(row),
                Err(reason) => invalid.push(ImportRowView {
                    index: offset,
                    start_at: 0,
                    end_at: 0,
                    seconds: 0,
                    title: String::new(),
                    category_name: None,
                    status: "invalid",
                    reason: Some(reason),
                    conflict_ids: Vec::new(),
                }),
            }
        }

        let (range_start, range_end) = match (
            rows.iter().map(|row| row.start_at).min(),
            rows.iter().map(|row| row.end_at).max(),
        ) {
            (Some(start), Some(end)) => (start, end),
            _ => (0, 0),
        };
        let existing = if rows.is_empty() {
            Vec::new()
        } else {
            self.store.day_segments(range_start, range_end)?
        };

        let mut accepted: Vec<(i64, i64, usize)> = Vec::new();
        let mut views = Vec::new();
        let mut importable = 0_u64;
        let mut conflicts = 0_u64;

        for row in &mut rows {
            let mut conflict_ids: Vec<String> = existing
                .iter()
                .filter(|segment| overlaps(segment.start_at_ms, segment.end_at_ms, row.start_at, row.end_at))
                .map(|segment| segment.id.clone())
                .collect();
            let mut row_conflict = !conflict_ids.is_empty();
            for (start, end, index) in &accepted {
                if *start < row.end_at && *end > row.start_at {
                    conflict_ids.push(format!("第 {index} 行"));
                    row_conflict = true;
                }
            }

            let status = if row_conflict { "conflict" } else { "ok" };
            row.status = status;
            if row_conflict {
                conflicts += 1;
            } else {
                importable += 1;
                accepted.push((row.start_at, row.end_at, row.index));
            }
            views.push(ImportRowView {
                index: row.index,
                start_at: row.start_at,
                end_at: row.end_at,
                seconds: (row.end_at - row.start_at) / 1_000,
                title: row.title.clone(),
                category_name: row.category_name.clone(),
                status,
                reason: None,
                conflict_ids,
            });
        }

        let invalid_count = invalid.len() as u64;
        views.extend(invalid);

        let total = views.len() as u64;
        Ok(ImportAnalysis {
            rows,
            preview: ImportPreviewView {
                total,
                importable,
                conflicts,
                invalid: invalid_count,
                rows: views,
            },
        })
    }

    fn category_ids(&self) -> AppResult<HashMap<String, String>> {
        Ok(self
            .store
            .categories()?
            .into_iter()
            .map(|row| (row.name, row.id))
            .collect())
    }
}

struct ImportAnalysis {
    rows: Vec<ParsedRow>,
    preview: ImportPreviewView,
}

fn overlaps(start: i64, end: Option<i64>, other_start: i64, other_end: i64) -> bool {
    start < other_end && end.unwrap_or(i64::MAX) > other_start
}

struct HeaderColumns {
    date: Option<usize>,
    start: usize,
    end: usize,
    title: Option<usize>,
    category: Option<usize>,
    note: Option<usize>,
    is_focus: Option<usize>,
    is_ignored: Option<usize>,
}

impl HeaderColumns {
    fn new(header: &[String]) -> AppResult<Self> {
        let mut index = HashMap::new();
        for (position, name) in header.iter().enumerate() {
            index.insert(name.trim().to_ascii_lowercase(), position);
        }
        let required = |name: &str| {
            index.get(name).copied().ok_or_else(|| {
                AppError::store("E_INVALID_RANGE", format!("CSV 缺少必需列 {name}"))
            })
        };
        Ok(Self {
            date: index.get("date").copied(),
            start: required("start")?,
            end: required("end")?,
            title: index.get("title").copied(),
            category: index.get("category").copied(),
            note: index.get("note").copied(),
            is_focus: index.get("is_focus").copied(),
            is_ignored: index.get("is_ignored").copied(),
        })
    }

    fn parse(&self, record: &[String], index: usize, tz: i32) -> Result<ParsedRow, String> {
        let field = |position: Option<usize>| -> String {
            position
                .and_then(|position| record.get(position))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        let date = field(self.date);
        let start_text = field(Some(self.start));
        let end_text = field(Some(self.end));
        if start_text.is_empty() || end_text.is_empty() {
            return Err("start / end 为空".to_string());
        }

        let (start_at, _) = parse_time(&start_text, &date, tz)?;
        let (mut end_at, end_had_date) = parse_time(&end_text, &date, tz)?;
        if !end_had_date && end_at < start_at {
            end_at += MS_PER_DAY; // 只给时刻时视为跨零点。
        }
        if end_at <= start_at {
            return Err("end 必须晚于 start".to_string());
        }
        if end_at - start_at > MAX_ROW_MS {
            return Err("单行时长超过 24 小时".to_string());
        }

        Ok(ParsedRow {
            index,
            start_at,
            end_at,
            title: field(self.title),
            note: field(self.note),
            category_name: Some(field(self.category)).filter(|value| !value.is_empty()),
            is_focus: truthy(&field(self.is_focus)),
            is_ignored: truthy(&field(self.is_ignored)),
            status: "ok",
        })
    }
}

/// 返回 `(utc_ms, 是否自带日期)`。
fn parse_time(text: &str, date: &str, tz: i32) -> Result<(i64, bool), String> {
    if let Some(ms) = parse_iso_local(text, tz) {
        return Ok((ms, text.contains(['T', ' '])));
    }
    if date.is_empty() {
        return Err(format!("无法解析时间 {text}"));
    }
    parse_date_time(date, text, tz)
        .map(|ms| (ms, false))
        .ok_or_else(|| format!("无法解析时间 {text}"))
}

fn truthy(text: &str) -> bool {
    matches!(text.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// 引号感知的记录切分：支持 `""` 转义与字段内换行。
fn parse_records(content: &str) -> Vec<Vec<String>> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                record.push(std::mem::take(&mut field));
            }
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            '\n' if !in_quotes => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            other => field.push(other),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_records() {
        let records = parse_records("\u{feff}a,\"b,c\",d\r\n1,\"say \"\"hi\"\"\",3\n");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], vec!["a", "b,c", "d"]);
        assert_eq!(records[1], vec!["1", "say \"hi\"", "3"]);
    }
}
