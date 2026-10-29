//! 轻量本地日志：每日一个文件，写入失败时静默降级，不阻塞主流程。
//!
//! 不引入 `tracing` 等外部依赖（见 `docs/architecture.md` 的依赖约束）；
//! 日志只用于诊断包与故障排查，不参与业务逻辑。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 保留的日志文件数量；更早的按文件名日期删除。
pub const KEEP_FILES: usize = 7;

pub fn log_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("logs")
}

fn file_path(app_dir: &Path) -> PathBuf {
    log_dir(app_dir).join(format!(
        "dotlog-{}.log",
        chrono::Local::now().format("%Y-%m-%d")
    ))
}

/// 追加一行日志；换行会被压平，保证一行一条事实。
pub fn write(app_dir: &Path, level: &str, message: &str) {
    let dir = log_dir(app_dir);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = format!(
        "{} [{}] {}\n",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z"),
        level,
        message.replace(['\r', '\n'], " ")
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path(app_dir))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 当天的最近 `limit` 行日志，供诊断包附带。
pub fn recent_lines(app_dir: &Path, limit: usize) -> Vec<String> {
    let Ok(content) = fs::read_to_string(file_path(app_dir)) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    if lines.len() > limit {
        lines = lines.split_off(lines.len() - limit);
    }
    lines
}

/// 删除超出保留数量的旧日志（文件名按日期可排序）。
pub fn prune(app_dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(log_dir(app_dir)) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("dotlog-") && name.ends_with(".log"))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for path in &files[..files.len() - keep] {
        let _ = fs::remove_file(path);
    }
}
