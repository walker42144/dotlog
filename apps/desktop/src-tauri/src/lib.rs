use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dotlog_application::dto::{
    BackfillInput, CategoryView, DayView, HistoryView, ImportPreviewView, ImportResultView,
    PunchResultView, RecoveryInfoView, SegmentPatch, SegmentView, StatsView, TimerStateView,
    UndoResultView,
};
use dotlog_application::{AppError, Clock, ExportFormat, Granularity, TimerService, TimerStore};
use dotlog_domain::recovery::RecoveryAction;
use dotlog_domain::{DomainError, Instant, PointSource, PunchMode};
use dotlog_plugin_sdk::CloseReason;
use dotlog_storage::Database;
use serde::{Deserialize, Serialize};

mod backup;
mod diagnostics;
mod integrations;
mod local_plugins;
mod log;
use backup::BackupEntry;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartExt};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// 前端订阅的快捷事件：托盘「打点」与全局快捷键共用，由前端按当前状态决定开始还是打点。
const QUICK_PUNCH_EVENT: &str = "dotlog://quick-punch";
/// 启动检测到异常时通知前端（前端也会主动调用 `recovery_get` 兜底）。
const RECOVERY_EVENT: &str = "recovery://required";
const HOTKEY_MODIFIERS: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT);
const HOTKEY_CODE: Code = Code::Space;

/// 关窗只隐藏到托盘，只有托盘「退出」才真正结束进程。
static QUITTING: AtomicBool = AtomicBool::new(false);

/// 系统本地时钟：毫秒时间戳 + 本地时区偏移（分钟）。
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        let now = chrono::Local::now();
        Instant::new(now.timestamp_millis(), now.offset().local_minus_utc() / 60)
    }
}

/// 启动核对结果只在启动时可得：新会话会覆盖心跳，因此必须缓存。
struct RecoveryCache {
    info: RecoveryInfoView,
    /// `truncate` 的落点；随缓存一起保存，`recovery_resolve` 直接复用。
    cutoff_ms: Option<i64>,
}

struct AppState {
    timer: Arc<TimerService<Database, SystemClock>>,
    /// 与 timer 共享同一连接的仓储句柄：插件宿主与 outbox 查询用。
    store: Arc<dyn TimerStore>,
    /// 插件宿主（M5）：集成启停、事件分发、outbox worker。
    host: Arc<dotlog_plugin_host::host::PluginHost>,
    recovery: Mutex<RecoveryCache>,
}

/// 与 `docs/ipc-contract.md` §5 对齐的错误信封；`E_OVERLAP` 携带冲突段明细。
#[derive(Serialize)]
struct IpcError {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

impl From<AppError> for IpcError {
    fn from(error: AppError) -> Self {
        let detail = match &error {
            AppError::Domain(DomainError::Overlap(conflicts)) => Some(serde_json::json!({
                "conflicts": conflicts
                    .iter()
                    .map(|segment| serde_json::json!({
                        "id": segment.id,
                        "startAt": segment.start_at_ms,
                        "endAt": segment.end_at_ms,
                        "title": segment.title,
                    }))
                    .collect::<Vec<_>>()
            })),
            _ => None,
        };
        Self {
            code: error.code(),
            message: error.to_string(),
            detail,
        }
    }
}

type IpcResult<T> = Result<T, IpcError>;

fn store_error(code: &'static str, message: String) -> IpcError {
    IpcError {
        code,
        message,
        detail: None,
    }
}

fn fs_error(error: std::io::Error) -> IpcError {
    store_error("E_UNKNOWN", format!("文件写入失败：{error}"))
}

fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dotlog")
}

/// `%APPDATA%\dotlog\dotlog.db`（见 `docs/data-dictionary.md`）。
fn db_path() -> PathBuf {
    app_dir().join("dotlog.db")
}

/// 导出文件目录；备份目录见 `backup::backup_dir`。
fn exports_dir() -> PathBuf {
    app_dir().join("exports")
}

/// 从合并设置读取滚动备份的保留数量。
fn retention(timer: &TimerService<Database, SystemClock>) -> (usize, usize, usize) {
    let settings = timer
        .settings(Some("app"))
        .unwrap_or_else(|_| serde_json::json!({}));
    let pick = |key: &str, fallback: usize| {
        settings
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .filter(|value| (1..=60).contains(value))
            .map(|value| value as usize)
            .unwrap_or(fallback)
    };
    (
        pick("backup.keep_daily", 7),
        pick("backup.keep_weekly", 4),
        pick("backup.keep_monthly", 6),
    )
}

/// 快照到 `backups/` 并执行保留清理；返回新文件信息。
fn create_backup(timer: &TimerService<Database, SystemClock>) -> IpcResult<BackupEntry> {
    let dir = backup::backup_dir(&app_dir());
    std::fs::create_dir_all(&dir)
        .map_err(|error| store_error("E_BACKUP_FAILED", format!("无法创建备份目录：{error}")))?;
    let path = backup::new_backup_path(&app_dir());
    timer.backup_to(&path).map_err(IpcError::from)?;

    let entry = backup::list(&dir)
        .into_iter()
        .find(|entry| Path::new(&entry.path) == path)
        .ok_or_else(|| store_error("E_BACKUP_FAILED", "备份文件未生成".to_string()))?;
    let (daily, weekly, monthly) = retention(timer);
    backup::prune(&dir, daily, weekly, monthly);
    Ok(entry)
}

/// 启动时的「每天首次使用」备份；失败只记录，不阻塞启动。
fn backup_on_startup(timer: &TimerService<Database, SystemClock>) {
    let enabled = timer
        .settings(Some("app"))
        .ok()
        .and_then(|settings| settings.get("backup.enabled")?.as_bool())
        .unwrap_or(true);
    if !enabled {
        return;
    }
    let dir = backup::backup_dir(&app_dir());
    if backup::has_backup_today(&dir) {
        return;
    }
    if let Err(error) = create_backup(timer) {
        eprintln!("启动备份失败：{}", error.message);
    }
}

/// 读取设置里的全局快捷键；无法解析时回落到默认组合。
fn configured_shortcut(timer: &TimerService<Database, SystemClock>) -> Shortcut {
    let configured = timer
        .settings(Some("app"))
        .ok()
        .and_then(|settings| settings.get("hotkey.punch")?.as_str().map(str::to_string));
    configured
        .and_then(|text| text.parse::<Shortcut>().ok())
        .unwrap_or_else(|| Shortcut::new(Some(HOTKEY_MODIFIERS), HOTKEY_CODE))
}

fn ensure_ready(state: &AppState) -> IpcResult<()> {
    let report = state.timer.check_session().map_err(IpcError::from)?;
    if !report.integrity.quick_check_ok
        || report.integrity.overlap_pairs > 0
        || report.integrity.active_count > 1
    {
        return Err(store_error(
            "E_DB_CORRUPT",
            "记录完整性检查未通过，请先备份并修复数据".into(),
        ));
    }
    if report.recovery.required {
        let mut cache = state.recovery.lock().map_err(|_| cache_error())?;
        if !cache.info.required {
            cache.info = report.recovery;
            cache.cutoff_ms = report.cutoff_ms;
        }
    }
    if state
        .recovery
        .lock()
        .map_err(|_| cache_error())?
        .info
        .required
    {
        return Err(store_error(
            "E_RECOVERY_REQUIRED",
            "请先处理计时恢复提示".into(),
        ));
    }
    Ok(())
}

fn quick_action(app: &AppHandle, pause: bool, source: PointSource) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if ensure_ready(&state).is_err() {
        show_main_window(app);
        return;
    }
    let result: IpcResult<Option<PunchResultView>> = (|| {
        let current = state.timer.get_state().map_err(IpcError::from)?;
        if current.active.is_some() {
            let result = if pause {
                state.timer.pause(source)
            } else {
                state.timer.punch(source)
            }
            .map_err(IpcError::from)?;
            if !pause {
                integrations::dispatch_closed(
                    &state,
                    &result.closed_segment.id,
                    CloseReason::Punch,
                );
            }
            Ok(Some(result))
        } else if pause {
            Ok(None)
        } else {
            state.timer.start(source).map_err(IpcError::from)?;
            Ok(None)
        }
    })();
    match result {
        Ok(payload) => {
            let _ = app.emit(QUICK_PUNCH_EVENT, payload);
        }
        Err(error) => {
            log::write(&app_dir(), "WARN", &error.message);
            show_main_window(app);
        }
    }
}

fn source_or_button(source: Option<PointSource>) -> PointSource {
    source.unwrap_or(PointSource::Button)
}

/// 唤起主窗口：单实例二次启动与托盘左键点击共用。
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn timer_get_state(state: tauri::State<'_, AppState>) -> IpcResult<TimerStateView> {
    state.timer.get_state().map_err(IpcError::from)
}

#[tauri::command]
fn timer_start(
    state: tauri::State<'_, AppState>,
    source: Option<PointSource>,
) -> IpcResult<TimerStateView> {
    ensure_ready(state.inner())?;
    state
        .timer
        .start(source_or_button(source))
        .map_err(IpcError::from)
}

#[tauri::command]
fn timer_punch(
    state: tauri::State<'_, AppState>,
    source: Option<PointSource>,
) -> IpcResult<PunchResultView> {
    ensure_ready(state.inner())?;
    let result = state
        .timer
        .punch(source_or_button(source))
        .map_err(IpcError::from)?;
    integrations::dispatch_closed(state.inner(), &result.closed_segment.id, CloseReason::Punch);
    Ok(result)
}

#[tauri::command]
fn timer_pause(
    state: tauri::State<'_, AppState>,
    source: Option<PointSource>,
) -> IpcResult<PunchResultView> {
    ensure_ready(state.inner())?;
    let result = state
        .timer
        .pause(source_or_button(source))
        .map_err(IpcError::from)?;

    Ok(result)
}

#[tauri::command]
fn timer_finish(
    state: tauri::State<'_, AppState>,
    source: Option<PointSource>,
) -> IpcResult<PunchResultView> {
    ensure_ready(state.inner())?;
    let result = state
        .timer
        .finish(source_or_button(source))
        .map_err(IpcError::from)?;
    integrations::dispatch_closed(
        state.inner(),
        &result.closed_segment.id,
        CloseReason::Finish,
    );
    Ok(result)
}

#[tauri::command]
fn timer_undo(state: tauri::State<'_, AppState>) -> IpcResult<UndoResultView> {
    ensure_ready(state.inner())?;
    state.timer.undo().map_err(IpcError::from)
}

#[tauri::command]
fn day_today(state: tauri::State<'_, AppState>) -> IpcResult<DayView> {
    state.timer.day(None).map_err(IpcError::from)
}

#[tauri::command]
fn day_get(state: tauri::State<'_, AppState>, date: String) -> IpcResult<DayView> {
    state.timer.day(Some(&date)).map_err(IpcError::from)
}

#[tauri::command]
fn pending_note_get(state: tauri::State<'_, AppState>) -> IpcResult<Option<SegmentView>> {
    state.timer.pending_note().map_err(IpcError::from)
}
#[tauri::command]
fn segment_confirm(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    state.timer.confirm_session(&id).map_err(IpcError::from)
}

#[tauri::command]
fn segment_get(state: tauri::State<'_, AppState>, id: String) -> IpcResult<SegmentView> {
    let segment = state
        .store
        .find_segment(&id)
        .map_err(IpcError::from)?
        .ok_or_else(|| store_error("E_SEGMENT_NOT_FOUND", "记录不存在".into()))?;
    let mut view = SegmentView::from_segment(&segment, chrono::Local::now().timestamp_millis());
    view.category = state
        .timer
        .categories()
        .map_err(IpcError::from)?
        .into_iter()
        .find(|c| Some(&c.id) == segment.category_id.as_ref());
    Ok(view)
}

#[tauri::command]
fn segment_update(
    state: tauri::State<'_, AppState>,
    id: String,
    revision: u32,
    patch: SegmentPatch,
) -> IpcResult<SegmentView> {
    ensure_ready(state.inner())?;
    state
        .timer
        .update_segment(&id, revision, patch)
        .map_err(IpcError::from)
}

#[tauri::command]
fn segment_backfill(
    state: tauri::State<'_, AppState>,
    input: BackfillInput,
) -> IpcResult<SegmentView> {
    ensure_ready(state.inner())?;
    let view = state
        .timer
        .backfill_segment(input)
        .map_err(IpcError::from)?;
    // 补记生成的是已关闭段，同样进入插件事件流。
    integrations::dispatch_closed(state.inner(), &view.id, CloseReason::Punch);
    Ok(view)
}

#[tauri::command]
fn segment_delete(state: tauri::State<'_, AppState>, id: String) -> IpcResult<()> {
    ensure_ready(state.inner())?;
    state.timer.delete_segment(&id).map_err(IpcError::from)
}

#[tauri::command]
fn segment_split(
    state: tauri::State<'_, AppState>,
    id: String,
    at: i64,
) -> IpcResult<Vec<SegmentView>> {
    ensure_ready(state.inner())?;
    state.timer.split_segment(&id, at).map_err(IpcError::from)
}

#[tauri::command]
fn segment_merge(state: tauri::State<'_, AppState>, ids: Vec<String>) -> IpcResult<SegmentView> {
    ensure_ready(state.inner())?;
    state.timer.merge_segments(&ids).map_err(IpcError::from)
}

#[tauri::command]
fn history_query(
    state: tauri::State<'_, AppState>,
    from: i64,
    to: i64,
    text: Option<String>,
    limit: u32,
    offset: u32,
) -> IpcResult<HistoryView> {
    state
        .timer
        .history(from, to, text.as_deref(), limit, offset)
        .map_err(IpcError::from)
}

#[tauri::command]
fn stats_get(
    state: tauri::State<'_, AppState>,
    from: i64,
    to: i64,
    granularity: Granularity,
) -> IpcResult<StatsView> {
    state
        .timer
        .stats(from, to, granularity)
        .map_err(IpcError::from)
}

#[tauri::command]
fn categories_list(state: tauri::State<'_, AppState>) -> IpcResult<Vec<CategoryView>> {
    state.timer.categories().map_err(IpcError::from)
}

/// `settings_patch` 的单项覆盖。
#[derive(Debug, Deserialize)]
struct SettingEntry {
    key: String,
    value: serde_json::Value,
}

#[tauri::command]
fn settings_get(
    state: tauri::State<'_, AppState>,
    scope: Option<String>,
) -> IpcResult<serde_json::Value> {
    state
        .timer
        .settings(scope.as_deref())
        .map_err(IpcError::from)
}

#[tauri::command]
fn settings_patch(
    state: tauri::State<'_, AppState>,
    scope: String,
    entries: Vec<SettingEntry>,
) -> IpcResult<serde_json::Value> {
    let entries = entries
        .into_iter()
        .map(|entry| (entry.key, entry.value))
        .collect();
    state
        .timer
        .settings_patch(&scope, entries)
        .map_err(IpcError::from)
}

#[tauri::command]
fn settings_reset(
    state: tauri::State<'_, AppState>,
    scope: String,
    key: Option<String>,
) -> IpcResult<()> {
    state
        .timer
        .settings_reset(&scope, key.as_deref())
        .map_err(IpcError::from)
}

#[derive(Serialize)]
struct ExportResult {
    path: String,
    count: u64,
}

#[tauri::command]
fn export_data(
    state: tauri::State<'_, AppState>,
    format: ExportFormat,
    from: i64,
    to: i64,
    path: Option<String>,
) -> IpcResult<ExportResult> {
    let payload = state
        .timer
        .export(format, from, to)
        .map_err(IpcError::from)?;
    let extension = match format {
        ExportFormat::Json => "json",
        ExportFormat::Csv => "csv",
    };
    let target = match path {
        Some(text) if !text.trim().is_empty() => PathBuf::from(text),
        _ => {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            exports_dir().join(format!("dotlog-{stamp}.{extension}"))
        }
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(fs_error)?;
    }
    std::fs::write(&target, payload.content).map_err(fs_error)?;
    Ok(ExportResult {
        path: target.display().to_string(),
        count: payload.count,
    })
}

#[tauri::command]
fn import_preview(
    state: tauri::State<'_, AppState>,
    content: String,
) -> IpcResult<ImportPreviewView> {
    state.timer.import_preview(&content).map_err(IpcError::from)
}

#[tauri::command]
fn import_commit(
    state: tauri::State<'_, AppState>,
    content: String,
) -> IpcResult<ImportResultView> {
    ensure_ready(state.inner())?;
    state.timer.import_commit(&content).map_err(IpcError::from)
}

#[tauri::command]
fn backup_now(state: tauri::State<'_, AppState>) -> IpcResult<BackupEntry> {
    create_backup(&state.timer)
}

#[tauri::command]
fn backup_list() -> IpcResult<Vec<BackupEntry>> {
    Ok(backup::list(&backup::backup_dir(&app_dir())))
}

#[tauri::command]
fn backup_restore(state: tauri::State<'_, AppState>, path: String) -> IpcResult<()> {
    let _lifecycle = state.host.lifecycle.lock().map_err(|_| cache_error())?;
    // Safety copies are not subject to rolling retention cleanup.
    let safe_dir = app_dir().join("restore-safety");
    std::fs::create_dir_all(&safe_dir).map_err(fs_error)?;
    let safety = safe_dir.join(format!("before-restore-{}.db", uuid_stamp()));
    state.timer.backup_to(&safety).map_err(IpcError::from)?;
    state
        .timer
        .restore_from(Path::new(&path))
        .map_err(IpcError::from)?;
    let report = state.timer.begin_session().map_err(IpcError::from)?;
    let mut cache = state.recovery.lock().map_err(|_| cache_error())?;
    cache.info = report.recovery;
    cache.cutoff_ms = report.cutoff_ms;
    Ok(())
}

fn uuid_stamp() -> String {
    dotlog_application::new_id()
}

fn cache_error() -> IpcError {
    store_error("E_UNKNOWN", "恢复状态不可用".to_string())
}

#[tauri::command]
fn recovery_get(state: tauri::State<'_, AppState>) -> IpcResult<RecoveryInfoView> {
    let cache = state.recovery.lock().map_err(|_| cache_error())?;
    Ok(cache.info.clone())
}

#[tauri::command]
fn recovery_resolve(
    state: tauri::State<'_, AppState>,
    action: RecoveryAction,
) -> IpcResult<TimerStateView> {
    let (reason, cutoff_ms) = {
        let cache = state.recovery.lock().map_err(|_| cache_error())?;
        match cache.info.reason {
            Some(reason) if cache.info.required => (reason, cache.cutoff_ms),
            _ => return Err(store_error("E_UNKNOWN", "没有需要处理的恢复项".to_string())),
        }
    };

    let view = state
        .timer
        .resolve_recovery(reason, action, cutoff_ms)
        .map_err(IpcError::from)?;

    if let Ok(mut cache) = state.recovery.lock() {
        cache.info = RecoveryInfoView::none(chrono::Local::now().timestamp_millis());
        cache.cutoff_ms = None;
    }
    log::write(
        &app_dir(),
        "INFO",
        &format!("恢复处理完成：reason={reason:?} action={action:?}"),
    );
    Ok(view)
}

#[derive(Serialize)]
struct DiagnosticsResult {
    path: String,
}

#[tauri::command]
fn diagnostics_snapshot(state: tauri::State<'_, AppState>) -> IpcResult<DiagnosticsResult> {
    let view = state.timer.diagnostics().map_err(IpcError::from)?;
    let root = app_dir();
    let tail = log::recent_lines(&root, 200);
    let path = diagnostics::write_snapshot(&root, view, tail)
        .map_err(|error| store_error("E_UNKNOWN", format!("诊断包写入失败：{error}")))?;
    log::write(&root, "INFO", &format!("诊断包已生成：{}", path.display()));
    Ok(DiagnosticsResult {
        path: path.display().to_string(),
    })
}

#[tauri::command]
fn data_open_folder(kind: String) -> IpcResult<()> {
    let path = match kind.as_str() {
        "backups" => backup::backup_dir(&app_dir()),
        "exports" => exports_dir(),
        "restore-safety" => app_dir().join("restore-safety"),
        "data" => app_dir(),
        _ => return Err(store_error("E_SETTING_INVALID", "目录类型无效".into())),
    };
    std::fs::create_dir_all(&path).map_err(fs_error)?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map_err(fs_error)?;
    Ok(())
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);

    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let punch_item = MenuItem::with_id(app, "punch", "打点", true, None::<&str>)?;
    let pause_item = MenuItem::with_id(app, "pause", "暂停", true, None::<&str>)?;
    let autostart_item = CheckMenuItem::with_id(
        app,
        "autostart",
        "开机自启",
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &punch_item,
            &pause_item,
            &PredefinedMenuItem::separator(app)?,
            &autostart_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let autostart_handle = autostart_item.clone();
    let mut tray = TrayIconBuilder::with_id("dotlog-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("打点 DotLog")
        .on_menu_event(move |app, event| {
            let id: &str = event.id().as_ref();
            match id {
                "show" => show_main_window(app),
                "punch" => {
                    quick_action(app, false, PointSource::Tray);
                }
                "pause" => {
                    quick_action(app, true, PointSource::Tray);
                }
                "autostart" => {
                    let manager = app.autolaunch();
                    let enabled = manager.is_enabled().unwrap_or(false);
                    let result = if enabled {
                        manager.disable()
                    } else {
                        manager.enable()
                    };
                    if result.is_ok() {
                        let _ = autostart_handle.set_checked(!enabled);
                    }
                }
                "quit" => {
                    QUITTING.store(true, Ordering::SeqCst);
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Err(error) = state.timer.end_session() {
                            log::write(&app_dir(), "WARN", &format!("退出标记写入失败：{error}"));
                        }
                    }
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.build(app)?;
    Ok(())
}

pub fn run() {
    // 崩溃时把恐慌信息落到日志，便于事后诊断。
    let panic_root = app_dir();
    std::panic::set_hook(Box::new(move |info| {
        log::write(&panic_root, "ERROR", &format!("panic: {info}"));
    }));

    tauri::Builder::default()
        // 单实例插件必须最先注册：第二次启动时唤起已有窗口。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None::<Vec<&'static str>>,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // 只注册一个快捷键，按键按下即触发打点事件。
                    if event.state() == ShortcutState::Pressed {
                        quick_action(app, false, PointSource::Hotkey);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let now_ms = chrono::Local::now().timestamp_millis();
            let root = app_dir();
            log::prune(&root, log::KEEP_FILES);
            log::write(
                &root,
                "INFO",
                &format!("DotLog {} 启动", env!("CARGO_PKG_VERSION")),
            );

            let db = Database::open(db_path(), now_ms)?;
            // timer 与插件宿主共享同一写连接（Database::Clone 只复制 Arc）。
            let timer = Arc::new(TimerService::new(db.clone(), SystemClock, PunchMode::Stop));
            let store: Arc<dyn TimerStore> = Arc::new(db);
            let host = integrations::build_host(Arc::clone(&store));

            // 启动核对失败不能阻塞启动：降级为「无需恢复」，但必须留下日志。
            let (recovery_info, cutoff_ms) = match timer.begin_session() {
                Ok(report) => {
                    let integrity = report.integrity;
                    log::write(
                        &root,
                        if report.recovery.required { "WARN" } else { "INFO" },
                        &format!(
                            "启动核对：reason={:?} quick_check={} active={} overlaps={} long_open={}s",
                            report.recovery.reason,
                            integrity.quick_check_ok,
                            integrity.active_count,
                            integrity.overlap_pairs,
                            integrity.longest_open_seconds
                        ),
                    );
                    if !integrity.quick_check_ok {
                        log::write(&root, "ERROR", "完整性自检失败：quick_check 未通过");
                    }
                    (report.recovery, report.cutoff_ms)
                }
                Err(error) => {
                    log::write(&root, "ERROR", &format!("启动核对失败：{error}"));
                    (RecoveryInfoView::none(now_ms), None)
                }
            };

            backup_on_startup(&timer);
            let punch_shortcut = configured_shortcut(&timer);
            let notify_recovery = recovery_info.clone();
            app.manage(AppState {
                timer: Arc::clone(&timer),
                store: Arc::clone(&store),
                host: Arc::clone(&host),
                recovery: Mutex::new(RecoveryCache {
                    info: recovery_info,
                    cutoff_ms,
                }),
            });
            if notify_recovery.required {
                let _ = app.handle().emit(RECOVERY_EVENT, &notify_recovery);
            }

            // outbox worker：先补发一次（应用关闭期间到期的任务），再按 30 s 轮询。
            integrations::spawn_worker(host, app.handle().clone());

            // 心跳：每 30 秒一次，用于下次启动检测休眠与长间隔。
            let heartbeat = Arc::clone(&timer);
            let heartbeat_root = root.clone();
            let heartbeat_app = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                if let Some(state) = heartbeat_app.try_state::<AppState>() {
                    let mut cache = match state.recovery.lock() { Ok(c) => c, Err(_) => continue };
                    if cache.info.required { continue; }
                    match heartbeat.check_session() {
                        Ok(report) if report.recovery.required => {
                            cache.info = report.recovery;
                            cache.cutoff_ms = report.cutoff_ms;
                            let _ = heartbeat_app.emit(RECOVERY_EVENT, &cache.info);
                            continue;
                        }
                        Err(error) => { log::write(&heartbeat_root, "ERROR", &format!("运行时核对失败：{error}")); continue; }
                        _ => {}
                    }
                }
                backup_on_startup(&heartbeat);
                if let Err(error) = heartbeat.record_heartbeat() {
                    log::write(&heartbeat_root, "WARN", &format!("心跳写入失败：{error}"));
                }
            });

            if let Err(error) = app.global_shortcut().register(punch_shortcut) {
                log::write(&root, "WARN", &format!("全局快捷键注册失败：{error}"));
            }

            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && !QUITTING.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            timer_get_state,
            timer_start,
            timer_punch,
            timer_pause,
            timer_finish,
            timer_undo,
            day_today,
            day_get,
            pending_note_get,
            segment_confirm,
            segment_get,
            segment_update,
            segment_backfill,
            segment_delete,
            segment_split,
            segment_merge,
            history_query,
            stats_get,
            categories_list,
            settings_get,
            settings_patch,
            settings_reset,
            export_data,
            import_preview,
            import_commit,
            data_open_folder,
            backup_now,
            backup_list,
            backup_restore,
            recovery_get,
            recovery_resolve,
            diagnostics_snapshot,
            local_plugins::local_plugin_list,
            local_plugins::local_plugin_install,
            local_plugins::local_plugin_change,
            local_plugins::local_plugin_rpc,
            local_plugins::plugin_sidebar_get,
            local_plugins::plugin_sidebar_set,
            integrations::integration_list,
            integrations::integration_set_enabled,
            integrations::integration_set_installed,
            integrations::memos_browse,
            integrations::memos_save,
            integrations::memos_delete,
            integrations::integration_config_set,
            integrations::integration_secret_set,
            integrations::integration_test,
            integrations::outbox_list,
            integrations::outbox_retry,
            integrations::ai_suggest,
            integrations::ai_apply,
            integrations::tomatodo_scan,
            integrations::tomatodo_import,
            integrations::tomatodo_compare,
            integrations::tomatodo_analyze
        ])
        .run(tauri::generate_context!())
        .expect("DotLog 启动失败");
}
