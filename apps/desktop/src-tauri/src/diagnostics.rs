//! 诊断包落盘：把应用层诊断快照、版本信息与最近日志写成一个 JSON 文件。

use std::fs;
use std::path::{Path, PathBuf};

use dotlog_application::DiagnosticsView;
use serde_json::json;

pub fn diagnostics_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("diagnostics")
}

/// 写盘并返回文件路径；调用方负责把路径回报给界面。
pub fn write_snapshot(
    app_dir: &Path,
    diagnostics: DiagnosticsView,
    log_tail: Vec<String>,
) -> std::io::Result<PathBuf> {
    let dir = diagnostics_dir(app_dir);
    fs::create_dir_all(&dir)?;

    let generated_at = diagnostics.generated_at;
    let payload = json!({
        "generatedAt": generated_at,
        "app": {
            "name": "DotLog",
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "diagnostics": diagnostics,
        "logTail": log_tail,
    });

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("dotlog-diagnostics-{stamp}.json"));
    let bytes = serde_json::to_vec_pretty(&payload).map_err(std::io::Error::other)?;
    fs::write(&path, bytes)?;
    Ok(path)
}
