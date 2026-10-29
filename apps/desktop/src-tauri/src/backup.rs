//! 备份文件的命名、列举与滚动保留（`docs/data-dictionary.md` §8）。
//!
//! 快照本身由 `dotlog-storage` 用 `VACUUM INTO` 完成；本模块只处理目录与策略。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime};
use serde::Serialize;

/// `backup_list` 的条目（`docs/ipc-contract.md` §3.5）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub path: String,
    pub created_at: i64,
    pub size_bytes: u64,
}

const FILE_FORMAT: &str = "dotlog-%Y%m%d-%H%M%S-%f.db";
fn parse_stamp(name: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(name, FILE_FORMAT)
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(name, "dotlog-%Y%m%d-%H%M%S.db").ok())
}

pub fn backup_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("backups")
}

/// 生成 `backups/dotlog-YYYYMMDD-HHmmss.db`。
pub fn new_backup_path(app_dir: &Path) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d-%H%M%S-%f");
    backup_dir(app_dir).join(format!("dotlog-{stamp}.db"))
}

/// 按时间倒序列出备份；无法解析的目录项一律忽略。
pub fn list(dir: &Path) -> Vec<BackupEntry> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(NaiveDateTime, BackupEntry)> = read_dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let stamp = parse_stamp(name)?;
            let size_bytes = entry.metadata().ok()?.len();
            let created_at = stamp.and_local_timezone(Local).single()?.timestamp_millis();
            Some((
                stamp,
                BackupEntry {
                    path: path.display().to_string(),
                    created_at,
                    size_bytes,
                },
            ))
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// 滚动保留：最近 `keep_daily` 天、`keep_weekly` 周、`keep_monthly` 月各留一份。
pub fn prune(dir: &Path, keep_daily: usize, keep_weekly: usize, keep_monthly: usize) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(NaiveDateTime, PathBuf)> = read_dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let stamp = parse_stamp(name)?;
            Some((stamp, path))
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.0));

    let mut days: HashSet<NaiveDate> = HashSet::new();
    let mut weeks: HashSet<(i32, u32)> = HashSet::new();
    let mut months: HashSet<(i32, u32)> = HashSet::new();
    let mut kept: HashSet<PathBuf> = HashSet::new();
    for (stamp, path) in &files {
        let date = stamp.date();
        let week = (date.iso_week().year(), date.iso_week().week());
        let month = (date.year(), date.month());
        let mut keep = false;
        if days.len() < keep_daily && days.insert(date) {
            keep = true;
        }
        if weeks.len() < keep_weekly && weeks.insert(week) {
            keep = true;
        }
        if months.len() < keep_monthly && months.insert(month) {
            keep = true;
        }
        if keep {
            kept.insert(path.clone());
        }
    }

    for (_, path) in files {
        if !kept.contains(&path) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 是否已有当天的备份（用于启动时的「每天首次使用」触发）。
pub fn has_backup_today(dir: &Path) -> bool {
    let today = Local::now().date_naive();
    list(dir).first().is_some_and(|entry| {
        DateTime::from_timestamp_millis(entry.created_at)
            .map(|stamp| stamp.with_timezone(&Local).date_naive() == today)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_backup_names_are_parseable() {
        let path = new_backup_path(Path::new("."));
        assert!(parse_stamp(path.file_name().unwrap().to_str().unwrap()).is_some());
    }
    #[test]
    fn original_backup_names_remain_readable() {
        assert!(parse_stamp("dotlog-20261029-123456.db").is_some());
    }
}
