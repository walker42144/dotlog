//! 番茄Todo 数据读取与映射（`docs/plugin-sdk.md` §9）。
//!
//! 番茄Todo 用 `lowdb` 的 `FileSync` adapter，数据是纯明文 JSON：
//! `%APPDATA%\tomatodo\tomatodo_db.json`。本模块只做只读解析，不写回。
//!
//! 数据模型（从真实样本恢复）：
//! - `PCRecord[]`：专注记录，`name`=科目，`time`=分钟，`startDate`=开始(ms)，
//!   `createDate`=保存时间(ms)，`isComplete`=1 才是有效专注。
//! - `PCToDo[]`：待办，`name`=名称，`time`=-1 表示未设时长，`s2`=创建时间戳(字符串)。

use std::fs;
use std::path::Path;

use chrono::Local;
use serde::{Deserialize, Serialize};

/// 番茄Todo 数据库顶层结构（只取我们关心的字段，其余用 `#[serde(default)]` 忽略）。
#[derive(Debug, Clone, Deserialize)]
pub struct TomatodoDb {
    #[serde(default)]
    #[serde(rename = "PCRecord")]
    pub pc_record: Vec<FocusRecord>,
    #[serde(default)]
    #[serde(rename = "PCToDo")]
    pub pc_todo: Vec<TodoItem>,
    #[serde(default)]
    #[serde(rename = "PCDeletedTodo")]
    pub pc_deleted_todo: Vec<TodoItem>,
    #[serde(default)]
    #[serde(rename = "recordIdCounter")]
    pub record_id_counter: i64,
    #[serde(default)]
    #[serde(rename = "todoIdCounter")]
    pub todo_id_counter: i64,
}

/// 番茄钟专注记录（`PCRecord` 数组元素）。
#[derive(Debug, Clone, Deserialize)]
pub struct FocusRecord {
    pub id: i64,
    /// 专注科目/内容名称。
    pub name: String,
    /// 专注分钟数（int，番茄Todo 记录的有效时长）。
    pub time: i64,
    /// 开始时刻，epoch 毫秒。
    #[serde(rename = "startDate")]
    pub start_date: i64,
    /// 记录创建/保存时刻，epoch 毫秒（略晚于 start+time，含操作延迟）。
    #[serde(rename = "createDate")]
    pub create_date: i64,
    /// 1=已完成的有效专注；0=中断/未完成，导入时跳过。
    #[serde(rename = "isComplete")]
    pub is_complete: i64,
    #[serde(default)]
    #[serde(rename = "isSynced")]
    pub is_synced: i64,
    #[serde(default)]
    #[serde(rename = "isTransfer")]
    pub is_transfer: i64,
    #[serde(default)]
    #[serde(rename = "boundDeviceId")]
    pub bound_device_id: Option<String>,
}

/// 待办事项（`PCToDo` 数组元素）。
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    #[serde(rename = "originalName")]
    pub original_name: Option<String>,
    /// 预估时长（分钟），-1 表示未设置。
    #[serde(default)]
    pub time: i64,
    #[serde(default)]
    #[serde(rename = "isComplied")]
    pub is_complied: i64,
    #[serde(default)]
    #[serde(rename = "repeatMode")]
    pub repeat_mode: i64,
    #[serde(default)]
    #[serde(rename = "sourceType")]
    pub source_type: Option<String>,
    /// 创建时间戳（epoch ms，但以字符串形式存储）。
    #[serde(default)]
    pub s2: Option<String>,
}

/// 扫描结果：返回给前端做预览，不写入 DotLog。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    /// 数据库文件路径（用户配置或默认）。
    pub db_path: String,
    /// 文件最后修改时间（epoch ms），用于判断是否需要重新扫描。
    pub file_mtime_ms: i64,
    /// `PCRecord` 总数。
    pub total_records: usize,
    /// `isComplete==1` 的有效专注数。
    pub completed_records: usize,
    /// 有效专注总分钟数。
    pub total_minutes: i64,
    /// 时间范围：最早开始时刻。
    pub earliest_start_ms: Option<i64>,
    /// 时间范围：最晚结束时刻。
    pub latest_end_ms: Option<i64>,
    /// 待办数。
    pub todo_count: usize,
    /// 预览：最近 10 条有效专注（按 startDate 降序）。
    pub preview: Vec<FocusPreview>,
}

/// 单条专注预览。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusPreview {
    pub id: i64,
    pub name: String,
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub duration_minutes: i64,
    pub start_text: String,
    pub end_text: String,
}

/// 导入结果统计。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: usize,
    pub skipped_existing: usize,
    pub skipped_incomplete: usize,
    pub conflicts: usize,
    pub errors: Vec<String>,
}

impl FocusRecord {
    /// 计算结束时刻：`startDate + time × 60_000`（用番茄Todo 记录的有效时长，
    /// 而不是墙钟 `createDate - startDate`，后者含暂停/操作延迟）。
    pub fn end_at_ms(&self) -> i64 {
        self.start_date + self.time * 60_000
    }

    /// 是否为有效完成的专注（导入条件）。
    pub fn is_completed(&self) -> bool {
        self.is_complete == 1 && self.time > 0
    }
}

impl TomatodoDb {
    /// 从 JSON 字符串解析。
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("JSON 解析失败：{error}"))
    }

    /// 从文件路径读取并解析。
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|error| format!("无法读取文件 {}：{error}", path.display()))?;
        Self::from_json(&content)
    }

    /// 筛选有效专注记录（`isComplete==1` 且 `time>0`）。
    pub fn completed_records(&self) -> Vec<&FocusRecord> {
        self.pc_record
            .iter()
            .filter(|record| record.is_completed())
            .collect()
    }
}

/// 扫描数据库文件，返回预览信息（不写入）。
pub fn scan(path: &Path) -> Result<ScanResult, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("无法读取文件元信息 {}：{error}", path.display()))?;
    let file_mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);

    let db = TomatodoDb::from_file(path)?;
    let completed = db.completed_records();
    let total_minutes: i64 = completed.iter().map(|r| r.time).sum();

    let mut sorted: Vec<&FocusRecord> = completed.clone();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.start_date));

    let preview: Vec<FocusPreview> = sorted
        .iter()
        .take(10)
        .map(|record| FocusPreview {
            id: record.id,
            name: record.name.clone(),
            start_at_ms: record.start_date,
            end_at_ms: record.end_at_ms(),
            duration_minutes: record.time,
            start_text: format_ms(record.start_date),
            end_text: format_ms(record.end_at_ms()),
        })
        .collect();

    let earliest = completed.iter().map(|r| r.start_date).min();
    let latest = completed.iter().map(|r| r.end_at_ms()).max();

    Ok(ScanResult {
        db_path: path.display().to_string(),
        file_mtime_ms,
        total_records: db.pc_record.len(),
        completed_records: completed.len(),
        total_minutes,
        earliest_start_ms: earliest,
        latest_end_ms: latest,
        todo_count: db.pc_todo.len(),
        preview,
    })
}

/// 把 epoch 毫秒格式化为本地 `YYYY-MM-DD HH:MM`。
pub(crate) fn format_ms(ms: i64) -> String {
    use chrono::TimeZone;
    match Local.timestamp_millis_opt(ms).single() {
        Some(time) => time.format("%Y-%m-%d %H:%M").to_string(),
        None => "--".into(),
    }
}
