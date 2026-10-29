//! 番茄Todo 集成插件（`docs/plugin-sdk.md` §9）。
//!
//! - 读取本机 `%APPDATA%\tomatodo\tomatodo_db.json`（番茄Todo 用 lowdb 明文存储）；
//! - 把 `PCRecord` 中 `isComplete==1` 的专注记录映射为 DotLog 段（startDate + time×60000）；
//! - 增量导入：记录上次导入的最大 record id，只导新增；同 startDate 自动跳过；
//! - 不联网、不写番茄Todo 文件、不接触其运行时——纯只读集成。
//!
//! 插件本身只声明 `UiPanel` 能力（出现在集成页）。实际写入 DotLog 段由桌面端的
//! `tomatodo_import` 命令调用 `backfill_segment` 完成，遵循「插件不写核心表」原则。

pub mod compare;
pub mod reader;

pub use compare::{ComparePair, CompareResult, CompareSummary, DotlogSegment, compare};
pub use reader::{FocusPreview, FocusRecord, ImportResult, ScanResult, TodoItem, TomatodoDb, scan};

use dotlog_plugin_sdk::{API_VERSION, Capability, Plugin, PluginManifest};
use serde_json::json;

/// 插件 id（manifest 契约的一部分，安装后不可更改）。
pub const PLUGIN_ID: &str = "tomatodo";

/// 设置中保存上次导入的最大 record id（增量游标）。
pub const SETTING_LAST_IMPORTED_ID: &str = "lastImportedId";
/// 设置中保存数据文件路径；留空则用默认 `%APPDATA%\tomatodo\tomatodo_db.json`。
pub const SETTING_DB_PATH: &str = "dbPath";
/// 设置中保存上次扫描的文件 mtime（epoch ms），用于判断是否有新数据。
pub const SETTING_LAST_MTIME: &str = "lastFileMtime";

/// 番茄Todo 内置插件：只读本地 JSON，不响应段关闭事件。
pub struct TomatodoPlugin;

impl Plugin for TomatodoPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: PLUGIN_ID.into(),
            name: "番茄Todo".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            // 只读本地文件 + 界面面板；不联网、不读密钥。
            capabilities: vec![Capability::UiPanel, Capability::SegmentsRead],
            network_hosts: Vec::new(),
            settings_schema: settings_schema(),
            entry: format!("builtin:{PLUGIN_ID}"),
        }
    }
}

/// 设置表单 schema（JSON Schema 子集，§2）。
fn settings_schema() -> serde_json::Value {
    json!({
        "dbPath": {
            "type": "string",
            "title": "数据库路径",
            "description": "留空则使用默认 %APPDATA%\\tomatodo\\tomatodo_db.json",
            "default": ""
        },
        "lastImportedId": {
            "type": "number",
            "title": "上次导入游标",
            "description": "自动维护，记录已导入的最大番茄钟记录 id",
            "default": 0
        },
        "lastFileMtime": {
            "type": "number",
            "title": "上次文件修改时间",
            "description": "自动维护，用于判断是否有新数据",
            "default": 0
        }
    })
}

/// 默认数据文件路径：`%APPDATA%\tomatodo\tomatodo_db.json`。
pub fn default_db_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA")
        .unwrap_or_else(|_| std::env::var("USERPROFILE").unwrap_or_default());
    std::path::Path::new(&appdata)
        .join("tomatodo")
        .join("tomatodo_db.json")
}

/// 解析用户配置的路径：为空则用默认路径。
pub fn resolve_db_path(configured: Option<&str>) -> std::path::PathBuf {
    match configured {
        Some(path) if !path.trim().is_empty() => std::path::PathBuf::from(path.trim()),
        _ => default_db_path(),
    }
}
