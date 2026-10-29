//! 设置用例：默认值、校验与持久化（`docs/ipc-contract.md` §3.4）。
//!
//! 数据库只保存覆盖值；应用 scope 的读取结果是「默认值 ⊕ 覆盖值」。

use dotlog_domain::DayPolicy;
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;

/// 应用设置命名空间。
pub const APP_SCOPE: &str = "app";

/// 设置项默认值；新增键必须同时补 `validate`。
pub fn default_settings() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("punch.mode".into(), Value::from("stop"));
    map.insert("punch.undo_window_ms".into(), Value::from(15_000));
    map.insert("punch.short_segment_ms".into(), Value::from(15_000));
    map.insert("day.start_minutes".into(), Value::from(240));
    map.insert(
        "day.active_window".into(),
        serde_json::json!({ "startMinutes": 240, "endMinutes": 1680 }),
    );
    map.insert("focus.min_minutes".into(), Value::from(10));
    map.insert("hotkey.punch".into(), Value::from("Ctrl+Shift+Space"));
    map.insert("notify.on_punch".into(), Value::from(true));
    map.insert("launch.at_login".into(), Value::from(false));
    map.insert("ui.theme".into(), Value::from("system"));
    map.insert("backup.enabled".into(), Value::from(true));
    map.insert("backup.keep_daily".into(), Value::from(7));
    map.insert("backup.keep_weekly".into(), Value::from(4));
    map.insert("backup.keep_monthly".into(), Value::from(6));
    map
}

/// 合并后的应用设置（含默认值）。
pub fn merged_settings(stored: &[(String, String)]) -> Value {
    let mut map = default_settings();
    for (key, raw) in stored {
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            map.insert(key.clone(), value);
        }
    }
    Value::Object(map)
}

/// 从合并设置中取 `day.start_minutes` 与 `focus.min_minutes`。
pub fn day_policy_of(settings: &Value) -> DayPolicy {
    let default = DayPolicy::default();
    let day_start_minutes = settings
        .get("day.start_minutes")
        .and_then(Value::as_i64)
        .map(|value| value.clamp(0, 1_439) as i32)
        .unwrap_or(default.day_start_minutes);
    let focus_min_minutes = settings
        .get("focus.min_minutes")
        .and_then(Value::as_i64)
        .map(|value| value.clamp(1, 480))
        .unwrap_or(default.focus_min_minutes);
    DayPolicy {
        day_start_minutes,
        focus_min_minutes,
    }
}

/// 校验单个设置项；`app` scope 的键必须已知且取值合法。
pub fn validate_entry(scope: &str, key: &str, value: &Value) -> AppResult<()> {
    if scope != APP_SCOPE {
        if ["token", "apiKey", "api_key", "password", "secret"].contains(&key) {
            return Err(AppError::store(
                "E_SETTING_INVALID",
                "敏感凭据必须使用系统钥匙串接口",
            ));
        }
        return Ok(());
    }
    let invalid = || AppError::store("E_SETTING_INVALID", format!("设置项 {key} 的值非法"));
    match key {
        "punch.mode" => match value.as_str() {
            Some("stop") | Some("chain") => Ok(()),
            _ => Err(invalid()),
        },
        "punch.undo_window_ms" => ranged(value, 1_000, 60_000).ok_or_else(invalid),
        "punch.short_segment_ms" => ranged(value, 1_000, 300_000).ok_or_else(invalid),
        "day.start_minutes" => ranged(value, 0, 1_439).ok_or_else(invalid),
        "day.active_window" => {
            let window = value.as_object().ok_or_else(invalid)?;
            let start = window.get("startMinutes").and_then(Value::as_i64);
            let end = window.get("endMinutes").and_then(Value::as_i64);
            match (start, end) {
                (Some(start), Some(end))
                    if (0..=2_880).contains(&start) && end > start && end - start <= 1_440 =>
                {
                    Ok(())
                }
                _ => Err(invalid()),
            }
        }
        "focus.min_minutes" => ranged(value, 1, 480).ok_or_else(invalid),
        "hotkey.punch" => match value.as_str() {
            Some(text) if !text.trim().is_empty() && text.len() <= 64 => Ok(()),
            _ => Err(invalid()),
        },
        "notify.on_punch" | "launch.at_login" | "backup.enabled" => {
            value.is_boolean().then_some(()).ok_or_else(invalid)
        }
        "ui.theme" => match value.as_str() {
            Some("light") | Some("dark") | Some("system") => Ok(()),
            _ => Err(invalid()),
        },
        "backup.keep_daily" | "backup.keep_weekly" | "backup.keep_monthly" => {
            ranged(value, 1, 60).ok_or_else(invalid)
        }
        _ => Err(AppError::store(
            "E_SETTING_INVALID",
            format!("未知设置项 {key}"),
        )),
    }
}

fn ranged(value: &Value, min: i64, max: i64) -> Option<()> {
    let number = value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.fract() == 0.0)
            .map(|number| number as i64)
    })?;
    (min..=max).contains(&number).then_some(())
}

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// `settings_get`：`app` scope 返回默认值合并结果。
    pub fn settings(&self, scope: Option<&str>) -> AppResult<Value> {
        let scope = scope.unwrap_or(APP_SCOPE);
        let stored = self.store.settings(scope)?;
        if scope == APP_SCOPE {
            return Ok(merged_settings(&stored));
        }
        let mut map = Map::new();
        for (key, raw) in stored {
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                map.insert(key, value);
            }
        }
        Ok(Value::Object(map))
    }

    /// `settings_patch`：逐项校验后写入，返回合并后的设置。
    pub fn settings_patch(&self, scope: &str, entries: Vec<(String, Value)>) -> AppResult<Value> {
        for (key, value) in &entries {
            validate_entry(scope, key, value)?;
        }
        let now_ms = self.clock.now().utc_ms;
        let rows: Vec<(String, String)> = entries
            .into_iter()
            .map(|(key, value)| {
                let raw = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
                (key, raw)
            })
            .collect();
        self.store.settings_upsert(scope, &rows, now_ms)?;
        self.settings(Some(scope))
    }

    /// `settings_reset`：删除覆盖值，回落到默认值。
    pub fn settings_reset(&self, scope: &str, key: Option<&str>) -> AppResult<()> {
        self.store.settings_reset(scope, key)
    }

    /// 当前生效的日窗口口径。
    pub(crate) fn day_policy(&self) -> AppResult<DayPolicy> {
        Ok(day_policy_of(&self.settings(Some(APP_SCOPE))?))
    }
}
