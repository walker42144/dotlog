//! Local UI plugins: persisted packages, explicit read/storage grants, no native execution.
use crate::{store_error, AppState, IpcError, IpcResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());
const SCOPE: &str = "local-plugins";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub description: String,
    pub author: String,
    pub permissions: Vec<String>,
    pub script: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub package: Package,
    pub enabled: bool,
    pub show_in_sidebar: bool,
}
fn valid_id(id: &str) -> bool {
    (3..=64).contains(&id.len())
        && id.starts_with("local-")
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
pub fn parse_package(text: &str) -> Result<Package, String> {
    if text.len() > 256 * 1024 {
        return Err("插件包不能超过256KB".into());
    }
    let p: Package = serde_json::from_str(text)
        .map_err(|_| "插件包不是合法的DotLog插件JSON，或包含不支持的字段")?;
    if !valid_id(&p.id) || p.api_version != 1 {
        return Err("插件ID必须以local-开头，apiVersion必须为1".into());
    }
    if p.name.trim().is_empty()
        || p.name.chars().count() > 40
        || p.description.len() > 2000
        || p.author.len() > 200
        || p.script.trim().is_empty()
    {
        return Err("插件名称、说明、作者或脚本不符合规范".into());
    }
    let parts: Vec<_> = p.version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|v| v.is_empty() || v.len() > 6 || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("版本格式应为1.0.0".into());
    }
    if p.permissions.len() > 3
        || p.permissions.iter().any(|v| {
            !matches!(
                v.as_str(),
                "segments:read" | "storage" | "tomatodo:link" | "tomatodo:history"
            )
        })
    {
        return Err("不支持的插件权限".into());
    }
    Ok(p)
}
fn fail(message: impl Into<String>) -> IpcError {
    store_error("E_PLUGIN", message.into())
}
fn read(state: &AppState, key: &str) -> IpcResult<Value> {
    let rows = state.store.settings(SCOPE).map_err(IpcError::from)?;
    match rows.iter().find(|(k, _)| k == key) {
        Some((_, value)) => {
            serde_json::from_str(value).map_err(|_| fail("插件配置损坏，请先导出诊断"))
        }
        None => Ok(Value::Null),
    }
}
fn write(state: &AppState, key: &str, value: &Value) -> IpcResult<()> {
    state
        .store
        .settings_upsert(
            SCOPE,
            &[(key.into(), value.to_string())],
            chrono::Local::now().timestamp_millis(),
        )
        .map_err(IpcError::from)
}
fn registry(state: &AppState) -> IpcResult<BTreeMap<String, Installed>> {
    let value = read(state, "registry")?;
    if value.is_null() {
        Ok(BTreeMap::new())
    } else {
        serde_json::from_value(value).map_err(|_| fail("插件注册表损坏"))
    }
}
fn save(state: &AppState, map: &BTreeMap<String, Installed>) -> IpcResult<()> {
    write(state, "registry", &json!(map))
}
#[tauri::command]
pub fn local_plugin_list(state: tauri::State<'_, AppState>) -> IpcResult<Vec<Installed>> {
    Ok(registry(&state)?.into_values().collect())
}
#[tauri::command]
pub fn local_plugin_install(
    state: tauri::State<'_, AppState>,
    text: String,
    replace: bool,
    show_in_sidebar: bool,
) -> IpcResult<Installed> {
    let p = parse_package(&text).map_err(fail)?;
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    let mut map = registry(&state)?;
    if map.contains_key(&p.id) && !replace {
        return Err(fail("同ID插件已安装，请明确选择替换更新"));
    }
    if map.len() >= 50 && !map.contains_key(&p.id) {
        return Err(fail("最多安装50个本地插件"));
    }
    // Updates also require a fresh enable action: newly requested grants never become active silently.
    if map.contains_key(&p.id) {
        super::tomatodo_link::stop(state.inner(), &p.id)?;
    }
    let installed = Installed {
        package: p,
        enabled: false,
        show_in_sidebar,
    };
    map.insert(installed.package.id.clone(), installed.clone());
    save(&state, &map)?;
    Ok(installed)
}
#[tauri::command]
pub fn local_plugin_change(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
    show_in_sidebar: bool,
    remove: bool,
) -> IpcResult<()> {
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    let mut map = registry(&state)?;
    let p = map.get_mut(&id).ok_or_else(|| fail("插件未安装"))?;
    let newly_enabled = enabled && !p.enabled;
    if enabled {
        parse_package(&serde_json::to_string(&p.package).map_err(|_| fail("插件清单转换失败"))?)
            .map_err(fail)?;
    }
    p.enabled = enabled;
    p.show_in_sidebar = show_in_sidebar;
    if remove {
        map.remove(&id);
    }
    if !enabled || remove {
        super::tomatodo_link::stop(state.inner(), &id)?;
    }
    save(&state, &map)?;
    if newly_enabled && !remove && link_authorized(&state, &id)? {
        set_auto_link(&state, &id, true)?;
    }
    Ok(())
}
#[tauri::command]
pub fn plugin_sidebar_get(state: tauri::State<'_, AppState>) -> IpcResult<Value> {
    read(&state, "sidebar")
}
#[tauri::command]
pub fn plugin_sidebar_set(
    state: tauri::State<'_, AppState>,
    id: String,
    visible: bool,
) -> IpcResult<()> {
    if !matches!(id.as_str(), "memos" | "ai" | "tomatodo") {
        return Err(fail("未知内置插件"));
    }
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    let mut value = read(&state, "sidebar")?;
    if value.is_null() {
        value = json!({});
    }
    value[&id] = json!(visible);
    write(&state, "sidebar", &value)
}
fn authorize(p: &Installed, method: &str) -> IpcResult<()> {
    if !p.enabled {
        return Err(fail("插件未启用"));
    }
    let permission = match method {
        "tomatodo.scan" | "tomatodo.import" | "tomatodo.compare" | "tomatodo.analyze" => {
            "tomatodo:history"
        }
        "day.get" => "segments:read",
        "storage.get" | "storage.set" => "storage",
        "link.status" | "link.enable" | "link.disable" | "link.startNext" | "link.endSegment"
        | "link.saveNote" | "link.skipNote" | "link.reconcile" => "tomatodo:link",
        _ => return Err(fail("插件API不支持此操作")),
    };
    if !p.package.permissions.iter().any(|v| v == permission) {
        return Err(fail("插件未获此权限"));
    }
    Ok(())
}
#[tauri::command]
pub async fn local_plugin_rpc(
    app: tauri::AppHandle,
    id: String,
    method: String,
    params: Value,
) -> IpcResult<Value> {
    use tauri::Manager;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        local_plugin_rpc_inner(state.inner(), &app, id, method, params)
    })
    .await
    .map_err(|e| fail(format!("插件操作失败：{e}")))?
}
fn local_plugin_rpc_inner(
    state: &AppState,
    app: &tauri::AppHandle,
    id: String,
    method: String,
    params: Value,
) -> IpcResult<Value> {
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    let p = registry(state)?
        .remove(&id)
        .ok_or_else(|| fail("插件未安装"))?;
    authorize(&p, &method)?;
    if let Some(action) = method.strip_prefix("link.") {
        return super::tomatodo_link::rpc(state, app, &id, action, &params);
    }
    if method.starts_with("tomatodo.") {
        if method == "tomatodo.import" {
            crate::ensure_ready(state)?;
        }
        if method != "tomatodo.import" {
            drop(_guard);
        }
        let result = match method.as_str() {
            "tomatodo.scan" => {
                serde_json::to_value(super::integrations::tomatodo_scan_inner(state)?)
            }
            "tomatodo.import" => {
                serde_json::to_value(super::integrations::tomatodo_import_inner(state)?)
            }
            "tomatodo.compare" => {
                serde_json::to_value(super::integrations::tomatodo_compare_inner(state)?)
            }
            "tomatodo.analyze" => {
                serde_json::to_value(super::integrations::tomatodo_analyze_inner(state)?)
            }
            _ => return Err(fail("不支持的历史操作")),
        };
        if method == "tomatodo.import" {
            use tauri::Emitter;
            let _ = app.emit("dotlog://link-changed", ());
        }
        return result.map_err(|_| fail("历史结果转换失败"));
    }
    match method.as_str() {
        "day.get" => {
            let date = params
                .get("date")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("缺少日期"))?;
            serde_json::to_value(state.timer.day(Some(date)).map_err(IpcError::from)?)
                .map_err(|_| fail("记录转换失败"))
        }
        "storage.get" => read(state, &format!("data:{id}")),
        "storage.set" => {
            if params.to_string().len() > 64 * 1024 {
                return Err(fail("每个插件的数据限64KB"));
            }
            write(state, &format!("data:{id}"), &params)?;
            Ok(Value::Null)
        }
        _ => unreachable!(),
    }
}

pub fn link_authorized(state: &AppState, id: &str) -> IpcResult<bool> {
    Ok(registry(state)?
        .get(id)
        .is_some_and(|p| authorize(p, "link.status").is_ok()))
}

// Called while LOCK is held, before taking the runtime gate. This is the same
// lock order as install/change/RPC, so disable and update cannot race restoration.
pub fn set_auto_link(state: &AppState, id: &str, enabled: bool) -> IpcResult<()> {
    let mut config = read(state, "tomatodo-autoconnect")?;
    if config.is_null() {
        config = json!({});
    }
    if !config.is_object() {
        return Err(fail("自动连接配置损坏，请先导出诊断"));
    }
    config[id] = json!(enabled);
    if enabled {
        config["preferred"] = json!(id);
    }
    write(state, "tomatodo-autoconnect", &config)
}

fn auto_link_candidate(map: &BTreeMap<String, Installed>, config: &Value) -> Option<String> {
    if !config.is_null() && !config.is_object() {
        return None;
    }
    let eligible = |id: &str, plugin: &Installed| {
        authorize(plugin, "link.status").is_ok()
            && config.get(id).and_then(Value::as_bool) != Some(false)
    };
    if let Some(id) = config.get("preferred").and_then(Value::as_str) {
        if map.get(id).is_some_and(|p| eligible(id, p)) {
            return Some(id.to_owned());
        }
    }
    let mut candidates = map.iter().filter(|(id, p)| eligible(id, p));
    let first = candidates.next()?.0.clone();
    candidates.next().is_none().then_some(first)
}

pub fn restore_auto_link(state: &AppState, app: &tauri::AppHandle) -> IpcResult<()> {
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    if super::tomatodo_link::enabled(state) {
        return Ok(());
    }
    let map = registry(state)?;
    let config = read(state, "tomatodo-autoconnect")?;
    if let Some(id) = auto_link_candidate(&map, &config) {
        let package = &map.get(&id).ok_or_else(|| fail("插件已移除"))?.package;
        parse_package(&serde_json::to_string(package).map_err(|_| fail("插件清单转换失败"))?)
            .map_err(fail)?;
        super::tomatodo_link::start(state, app, &id)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn link_plugin(id: &str, enabled: bool) -> Installed {
        let mut p = package();
        p["id"] = json!(id);
        p["permissions"] = json!(["tomatodo:link"]);
        Installed {
            package: parse_package(&p.to_string()).unwrap(),
            enabled,
            show_in_sidebar: true,
        }
    }
    #[test]
    fn automatic_link_requires_enabled_grant_and_respects_persistent_pause() {
        let mut map = BTreeMap::new();
        map.insert("local-link".into(), link_plugin("local-link", true));
        assert_eq!(
            auto_link_candidate(&map, &Value::Null).as_deref(),
            Some("local-link")
        );
        map.get_mut("local-link").unwrap().show_in_sidebar = false;
        assert_eq!(
            auto_link_candidate(&map, &Value::Null).as_deref(),
            Some("local-link")
        );
        assert_eq!(
            auto_link_candidate(&map, &json!({"local-link":false})),
            None
        );
        map.get_mut("local-link").unwrap().enabled = false;
        assert_eq!(auto_link_candidate(&map, &Value::Null), None);
        let p = map.get_mut("local-link").unwrap();
        p.enabled = true;
        p.package.permissions.clear();
        assert_eq!(auto_link_candidate(&map, &Value::Null), None);
    }
    #[test]
    fn multiple_automatic_links_require_a_valid_preferred_owner() {
        let map = BTreeMap::from([
            ("local-one".into(), link_plugin("local-one", true)),
            ("local-two".into(), link_plugin("local-two", true)),
        ]);
        assert_eq!(auto_link_candidate(&map, &Value::Null), None);
        assert_eq!(
            auto_link_candidate(&map, &json!({"preferred":"local-two"})).as_deref(),
            Some("local-two")
        );
        assert_eq!(
            auto_link_candidate(&map, &json!({"preferred":"local-two","local-two":false}))
                .as_deref(),
            Some("local-one")
        );
        assert_eq!(auto_link_candidate(&map, &json!([])), None);
    }
    fn package() -> Value {
        json!({"id":"local-demo", "name":"示例", "version":"1.0.0", "apiVersion":1, "author":"Demo", "description":"示例", "permissions":["storage"], "script":"DotLog.render({blocks:[]})"})
    }
    #[test]
    fn validates_supported_package() {
        assert!(parse_package(&package().to_string()).is_ok());
    }
    #[test]
    fn rejects_native_permissions_paths_and_versions() {
        for (key, value) in [
            ("id", json!("../memos")),
            ("permissions", json!(["network"])),
            ("apiVersion", json!(2)),
            ("version", json!("latest")),
            ("entry", json!("evil.exe")),
        ] {
            let mut p = package();
            p[key] = value;
            assert!(parse_package(&p.to_string()).is_err());
        }
    }
    #[test]
    fn checks_current_enable_state_and_grants_for_every_request() {
        let mut p = Installed {
            package: parse_package(&package().to_string()).unwrap(),
            enabled: false,
            show_in_sidebar: true,
        };
        assert!(authorize(&p, "storage.get").is_err());
        p.enabled = true;
        assert!(authorize(&p, "storage.get").is_ok());
        assert!(authorize(&p, "day.get").is_err());
        assert!(authorize(&p, "segment.delete").is_err());
        p.package.permissions.push("segments:read".into());
        assert!(authorize(&p, "day.get").is_ok());
    }
    #[test]
    fn enforces_package_limit() {
        assert!(parse_package(&" ".repeat(256 * 1024 + 1)).is_err());
    }
    #[test]
    fn link_grant_is_explicit_and_never_allows_raw_timer_or_observer_calls() {
        let mut p = Installed {
            package: parse_package(&package().to_string()).unwrap(),
            enabled: true,
            show_in_sidebar: true,
        };
        assert!(authorize(&p, "link.enable").is_err());
        p.package.permissions.push("tomatodo:link".into());
        for method in [
            "link.status",
            "link.enable",
            "link.disable",
            "link.startNext",
            "link.endSegment",
            "link.saveNote",
            "link.skipNote",
            "link.reconcile",
        ] {
            assert!(authorize(&p, method).is_ok());
        }
        for method in ["timer.start", "link.observe", "link.sql", "process.spawn"] {
            assert!(authorize(&p, method).is_err());
        }
        p.enabled = false;
        assert!(authorize(&p, "link.enable").is_err());
    }
    #[test]
    fn history_is_an_explicit_optional_grant_and_revocation_blocks_every_action() {
        let mut p = Installed {
            package: parse_package(&package().to_string()).unwrap(),
            enabled: true,
            show_in_sidebar: false,
        };
        p.package.permissions = vec!["tomatodo:link".into()];
        assert!(authorize(&p, "link.status").is_ok());
        assert!(authorize(&p, "tomatodo.import").is_err());
        p.package.permissions.push("tomatodo:history".into());
        for method in [
            "tomatodo.scan",
            "tomatodo.import",
            "tomatodo.compare",
            "tomatodo.analyze",
        ] {
            assert!(authorize(&p, method).is_ok());
        }
        for method in ["tomatodo.sql", "tomatodo.readFile", "process.spawn"] {
            assert!(authorize(&p, method).is_err());
        }
        p.enabled = false;
        for method in ["link.status", "tomatodo.scan", "tomatodo.import"] {
            assert!(authorize(&p, method).is_err());
        }
    }
}
