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
    if p.permissions.len() > 2
        || p.permissions
            .iter()
            .any(|v| !matches!(v.as_str(), "segments:read" | "storage"))
    {
        return Err("当前仅支持segments:read与storage权限".into());
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
    if enabled {
        parse_package(&serde_json::to_string(&p.package).map_err(|_| fail("插件清单转换失败"))?)
            .map_err(fail)?;
    }
    p.enabled = enabled;
    p.show_in_sidebar = show_in_sidebar;
    if remove {
        map.remove(&id);
    }
    save(&state, &map)
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
        "day.get" => "segments:read",
        "storage.get" | "storage.set" => "storage",
        _ => return Err(fail("插件API不支持此操作")),
    };
    if !p.package.permissions.iter().any(|v| v == permission) {
        return Err(fail("插件未获此权限"));
    }
    Ok(())
}
#[tauri::command]
pub fn local_plugin_rpc(
    state: tauri::State<'_, AppState>,
    id: String,
    method: String,
    params: Value,
) -> IpcResult<Value> {
    let _guard = LOCK.lock().map_err(|_| fail("插件锁不可用"))?;
    let p = registry(&state)?
        .remove(&id)
        .ok_or_else(|| fail("插件未安装"))?;
    authorize(&p, &method)?;
    match method.as_str() {
        "day.get" => {
            let date = params
                .get("date")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("缺少日期"))?;
            serde_json::to_value(state.timer.day(Some(date)).map_err(IpcError::from)?)
                .map_err(|_| fail("记录转换失败"))
        }
        "storage.get" => read(&state, &format!("data:{id}")),
        "storage.set" => {
            if params.to_string().len() > 64 * 1024 {
                return Err(fail("每个插件的数据限64KB"));
            }
            write(&state, &format!("data:{id}"), &params)?;
            Ok(Value::Null)
        }
        _ => unreachable!(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
}
