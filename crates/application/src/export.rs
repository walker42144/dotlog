//! 导出用例：生成 JSON / CSV 文本（`docs/data-dictionary.md` §7）。
//!
//! 写盘与路径选择由桌面层负责，应用层只产出内容。

use std::collections::HashMap;

use dotlog_domain::Segment;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::dto::ExportPayload;
use crate::error::{AppError, AppResult};
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;
use crate::timefmt::iso_local;

/// 导出格式（`export_data` 的 `format`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Json,
    Csv,
}

const CSV_HEADER: &str =
    "date,start,end,duration_min,title,category,tags,note,is_focus,is_ignored,source";

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// `export_data`：导出 `[from_ms, to_ms)` 内的段。
    pub fn export(
        &self,
        format: ExportFormat,
        from_ms: i64,
        to_ms: i64,
    ) -> AppResult<ExportPayload> {
        if from_ms >= to_ms {
            return Err(AppError::store("E_INVALID_RANGE", "导出区间起止非法"));
        }
        let segments = self.store.day_segments(from_ms, to_ms)?;
        let categories = self.store.categories()?;
        let names: HashMap<String, String> = categories
            .into_iter()
            .map(|row| (row.id, row.name))
            .collect();
        let now = self.clock.now();
        let policy = self.day_policy()?;

        let count = segments.len() as u64;
        let content = match format {
            ExportFormat::Json => self.export_json(&segments, &names, now.utc_ms, now.tz_offset_min)?,
            ExportFormat::Csv => {
                export_csv(&segments, &names, now.tz_offset_min, policy.day_start_minutes)
            }
        };
        Ok(ExportPayload { content, count })
    }

    fn export_json(
        &self,
        segments: &[Segment],
        names: &HashMap<String, String>,
        now_ms: i64,
        tz_offset_min: i32,
    ) -> AppResult<String> {
        let rows: Vec<Value> = segments
            .iter()
            .map(|segment| {
                json!({
                    "id": segment.id,
                    "start_at": iso_local(segment.start_at_ms, segment.tz_offset_min),
                    "end_at": segment.end_at_ms.map(|end| iso_local(end, segment.tz_offset_min)),
                    "title": segment.title,
                    "note": segment.note,
                    "category": segment
                        .category_id
                        .as_ref()
                        .and_then(|id| names.get(id))
                        .cloned()
                        .unwrap_or_default(),
                    "tags": segment.tags,
                    "is_focus": segment.is_focus,
                    "is_ignored": segment.is_ignored,
                    "source": "manual",
                })
            })
            .collect();

        let category_rows: Vec<Value> = names
            .iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect();

        let document = json!({
            "schema_version": 1,
            "app": { "name": "dotlog", "version": env!("CARGO_PKG_VERSION") },
            "exported_at": iso_local(now_ms, tz_offset_min),
            "data": {
                "segments": rows,
                "categories": category_rows,
                "tags": [],
                "settings": self.settings(Some(crate::settings::APP_SCOPE))?,
            }
        });
        Ok(serde_json::to_string_pretty(&document).unwrap_or_default())
    }
}

fn export_csv(
    segments: &[Segment],
    names: &HashMap<String, String>,
    tz_offset_min: i32,
    day_start_minutes: i32,
) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(CSV_HEADER);
    out.push_str("\r\n");
    for segment in segments {
        let end = segment.end_at_ms.unwrap_or(segment.start_at_ms);
        let seconds = (end - segment.start_at_ms) / 1_000;
        let category = segment
            .category_id
            .as_ref()
            .and_then(|id| names.get(id))
            .cloned()
            .unwrap_or_default();
        let row = [
            local_day(segment.start_at_ms, tz_offset_min, day_start_minutes),
            iso_local(segment.start_at_ms, segment.tz_offset_min),
            iso_local(end, segment.tz_offset_min),
            minutes_text(seconds),
            segment.title.clone(),
            category,
            segment.tags.join(";"),
            segment.note.clone(),
            bool_text(segment.is_focus),
            bool_text(segment.is_ignored),
            "manual".to_string(),
        ];
        out.push_str(&row.map(|field| csv_field(&field)).join(","));
        out.push_str("\r\n");
    }
    out
}

fn local_day(utc_ms: i64, tz_offset_min: i32, day_start_minutes: i32) -> String {
    dotlog_domain::local_date(utc_ms, tz_offset_min, day_start_minutes)
}

fn minutes_text(seconds: i64) -> String {
    let minutes = seconds as f64 / 60.0;
    let text = format!("{minutes:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn bool_text(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
