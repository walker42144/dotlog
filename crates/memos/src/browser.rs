//! Token-authenticated Memos client. Credentials stay in the desktop keyring bridge.
use dotlog_plugin_sdk::{HttpClient, HttpRequest};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memo {
    pub name: String,
    pub content: String,
    pub visibility: String,
    pub create_time: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoPage {
    pub user: String,
    pub user_id: String,
    pub instance_url: String,
    pub memos: Vec<Memo>,
    pub next_page_token: Option<String>,
    pub api_version: String,
}

pub struct BrowserClient<'a> {
    pub http: &'a dyn HttpClient,
    pub base: &'a str,
    pub token: &'a str,
}

impl BrowserClient<'_> {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", self.base);
        let response = self
            .http
            .send(HttpRequest {
                method,
                url: &url,
                headers: vec![
                    ("Authorization".into(), format!("Bearer {}", self.token)),
                    ("Content-Type".into(), "application/json".into()),
                ],
                body: body.map(|b| b.to_string().into_bytes()),
                timeout_ms: 10_000,
            })
            .map_err(|e| e.to_string())?;
        if matches!(response.status, 401 | 403) {
            return Err("Access Token 无效、已过期或权限不足，请在插件配置中更新".into());
        }
        let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        Ok((response.status, value))
    }

    pub fn authenticate(&self, preferred: &str) -> Result<(String, Value), String> {
        // Memos changed its identity endpoint in 0.25 and again in 0.26.
        // All v1 identity variants share the /api/v1/memos resource API.
        let mut endpoints = vec![
            ("v1", "GET", "/api/v1/auth/me"),
            ("v1", "GET", "/api/v1/auth/sessions/current"),
            ("v1", "POST", "/api/v1/auth/status"),
            ("current", "GET", "/api/auth/me"),
            ("v0", "GET", "/api/user/me"),
        ];
        if matches!(preferred, "current" | "v0") {
            endpoints.sort_by_key(|(version, _, _)| *version != preferred);
        }
        for (version, method, path) in endpoints {
            let (status, value) =
                self.request(method, path, (method == "POST").then(|| json!({})))?;
            if matches!(status, 404 | 405) || (status == 200 && value.is_null()) {
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(format!("Memos 身份验证失败（HTTP {status}）"));
            }
            let data = value.get("data").unwrap_or(&value);
            let user = data.get("user").unwrap_or(data);
            if user
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|s| s.starts_with("users/") && s.len() > 6)
                || user
                    .get("username")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            {
                return Ok((version.into(), user.clone()));
            }
        }
        Err("当前 Memos 版本没有返回可验证的用户信息，请检查实例地址和版本".into())
    }

    pub fn browse(&self, preferred: &str, page_token: Option<&str>) -> Result<MemoPage, String> {
        let (version, user) = self.authenticate(preferred)?;
        let path = match version.as_str() {
            "v0" => "/api/memo",
            "current" => "/api/memos",
            _ => "/api/v1/memos",
        };
        let page = page_token.unwrap_or("");
        let url = if version == "v0" {
            path.to_string()
        } else {
            format!("{path}?pageSize=50&pageToken={}", encode_query(page))
        };
        let (status, value) = self.request("GET", &url, None)?;
        if !(200..300).contains(&status) {
            return Err(format!("读取 Memos 失败（HTTP {status}）"));
        }
        let rows = value
            .get("memos")
            .or_else(|| value.get("data"))
            .unwrap_or(&value);
        let rows = if rows.is_null() && value.is_object() {
            Vec::new()
        } else {
            rows.as_array()
                .cloned()
                .ok_or("Memos 返回了非记录列表，未作为成功处理")?
        };
        let memos: Result<Vec<_>, _> = rows.iter().map(parse_memo).collect();
        let display = user
            .get("displayName")
            .or_else(|| user.get("nickname"))
            .or_else(|| user.get("username"))
            .or_else(|| user.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Memos 用户");
        Ok(MemoPage {
            user: display.into(),
            user_id: user
                .get("name")
                .or_else(|| user.get("username"))
                .and_then(Value::as_str)
                .unwrap_or(display)
                .into(),
            instance_url: self.base.into(),
            memos: memos?,
            next_page_token: value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            api_version: version,
        })
    }

    pub fn save(
        &self,
        preferred: &str,
        name: Option<&str>,
        content: &str,
        visibility: &str,
    ) -> Result<Memo, String> {
        if content.trim().is_empty() || content.chars().count() > 20_000 {
            return Err("内容不能为空或超过 20000 字".into());
        }
        let (version, _) = self.authenticate(preferred)?;
        let prefix = match version.as_str() {
            "v0" => "/api/memo",
            "current" => "/api/memos",
            _ => "/api/v1/memos",
        };
        let visibility = match visibility {
            "PUBLIC" => "PUBLIC",
            "PROTECTED" => "PROTECTED",
            _ => "PRIVATE",
        };
        let path = if let Some(name) = name {
            let id = memo_id(name)?;
            if version == "v0" {
                format!("{prefix}/{id}")
            } else {
                format!("{prefix}/{id}?updateMask=content,visibility")
            }
        } else {
            prefix.to_string()
        };
        let body = json!({"content": content.trim(), "visibility": visibility});
        let (status, value) = self.request(
            if name.is_some() { "PATCH" } else { "POST" },
            &path,
            Some(body),
        )?;
        if !(200..300).contains(&status) {
            return Err(format!(
                "保存 Memos 失败（HTTP {status}），请核对远端后重试"
            ));
        }
        parse_memo(
            value
                .get("memo")
                .or_else(|| value.get("data"))
                .unwrap_or(&value),
        )
    }

    pub fn delete(&self, preferred: &str, name: &str) -> Result<(), String> {
        let (version, _) = self.authenticate(preferred)?;
        let prefix = match version.as_str() {
            "v0" => "/api/memo",
            "current" => "/api/memos",
            _ => "/api/v1/memos",
        };
        let (status, _) = self.request("DELETE", &format!("{prefix}/{}", memo_id(name)?), None)?;
        if !(200..300).contains(&status) {
            return Err(format!("删除 Memos 失败（HTTP {status}）"));
        }
        Ok(())
    }
}

fn memo_id(name: &str) -> Result<&str, String> {
    let id = name.strip_prefix("memos/").unwrap_or(name);
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("记录标识无效".into());
    }
    Ok(id)
}

pub fn parse_memo(value: &Value) -> Result<Memo, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("id")
                .and_then(Value::as_i64)
                .map(|id| id.to_string())
        })
        .or_else(|| value.get("uid").and_then(Value::as_str).map(str::to_string))
        .ok_or("Memos 记录缺少标识")?;
    memo_id(&name)?;
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or("Memos 记录缺少正文")?;
    Ok(Memo {
        name,
        content: content.into(),
        visibility: value
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("PRIVATE")
            .into(),
        create_time: value
            .get("createTime")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("createdTs")
                    .and_then(Value::as_i64)
                    .map(|n| n.to_string())
            })
            .unwrap_or_default(),
    })
}

fn encode_query(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
