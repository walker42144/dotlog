//! Memos API 路径与版本探测封装（`docs/plugin-sdk.md` §7.2）。
//!
//! 所有 API 路径集中在此：后续 Memos v2 只需在此登记，不影响调用方。

use dotlog_plugin_sdk::{HttpClient, HttpRequest};

/// 探测候选路径，按优先级排序，覆盖不同 Memos 版本。
/// - v0.22+：`/api/v1/workspace/profile`（公开）、`/api/v1/memos`（需认证）
/// - v0.14-0.21：`/api/v1/status`（公开）
/// - v0.13 及更早：`/api/status`、`/api/memo`
const PROBE_CANDIDATES: &[(&str, &str)] = &[
    ("/api/v1/workspace/profile", "v1"),
    ("/api/v1/memos", "v1"),
    ("/api/v1/status", "v1"),
    ("/api/status", "v0"),
    ("/api/memo", "v0"),
];
const PROBE_TIMEOUT_MS: u64 = 5_000;

/// 探测结论；携带匹配到的 API 版本，供发布时选择路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 探测路径返回 2xx。
    Ok { api_version: &'static str },
    /// 路径存在（401/403/405）但认证失败或方法不允许。
    AuthIssue { api_version: &'static str },
}

impl ProbeOutcome {
    /// 适配的 API 版本；探测结果会写回设置 `apiVersion`。
    pub fn api_version(self) -> &'static str {
        match self {
            ProbeOutcome::Ok { api_version } | ProbeOutcome::AuthIssue { api_version } => {
                api_version
            }
        }
    }

    /// 连接测试成功后的提示文案。
    pub fn describe(self) -> &'static str {
        match self {
            ProbeOutcome::Ok { .. } => "连接成功",
            ProbeOutcome::AuthIssue { .. } => "实例可达，但令牌无效或权限不足",
        }
    }
}

/// 归一化实例地址：去尾部 `/`，仅接受 http(s)（§7.1）。
pub fn normalize_base(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let ok = trimmed.starts_with("https://") || trimmed.starts_with("http://");
    ok.then(|| trimmed.to_string())
}

/// 版本 → 发布路径；未登记的版本返回 `None`。
pub fn publish_path(api_version: &str) -> Option<&'static str> {
    match api_version {
        "v1" | "" => Some("/api/v1/memos"),
        "v0" => Some("/api/memo"),
        _ => None,
    }
}

/// 设置值 → Memos v1 proto 枚举大小写（`PRIVATE` / `PROTECTED` / `PUBLIC`）。
pub fn visibility_proto(visibility: &str) -> &'static str {
    match visibility {
        "protected" => "PROTECTED",
        "public" => "PUBLIC",
        _ => "PRIVATE",
    }
}

/// 单次探测的详细结果。
enum ProbeResult {
    /// 2xx 成功。
    Success,
    /// 路径存在但认证失败（401/403）或方法不允许（405）。
    PathExistsAuthIssue(u16),
    /// 路径不存在（404）。
    NotFound,
    /// 其他 HTTP 状态。
    HttpStatus(u16, String),
    /// 请求层失败（网络、超时、白名单等）。
    Network(String),
}

/// 连接测试：依次尝试多个候选路径，覆盖不同 Memos 版本。
/// `token` 可选；提供时附带 Authorization，便于提前发现令牌问题。
pub fn probe(
    http: &dyn HttpClient,
    base_url: &str,
    token: Option<&str>,
) -> Result<ProbeOutcome, String> {
    let Some(base) = normalize_base(base_url) else {
        return Err("实例地址无效，需要以 http(s):// 开头".into());
    };

    let mut not_found_paths: Vec<(&str, String)> = Vec::new();
    let mut auth_issue: Option<(&'static str, u16)> = None;
    let mut last_network_error: Option<String> = None;

    for (path, api_version) in PROBE_CANDIDATES {
        let url = format!("{base}{path}");
        match probe_once(http, &url, token) {
            ProbeResult::Success => {
                return Ok(ProbeOutcome::Ok { api_version });
            }
            ProbeResult::PathExistsAuthIssue(status) => {
                // 路径存在，只是认证/方法问题。记录第一个即可。
                if auth_issue.is_none() {
                    auth_issue = Some((api_version, status));
                }
            }
            ProbeResult::NotFound => {
                not_found_paths.push((path, "HTTP 404".to_string()));
            }
            ProbeResult::HttpStatus(status, body) => {
                not_found_paths.push((path, format!("HTTP {status}：{body}")));
            }
            ProbeResult::Network(detail) => {
                last_network_error = Some(detail);
            }
        }
    }

    // 找到了存在的路径但认证失败 → 报告认证问题
    if let Some((api_version, _status)) = auth_issue {
        return Ok(ProbeOutcome::AuthIssue { api_version });
    }

    // 全部 404 → API 版本不匹配
    if !not_found_paths.is_empty() {
        let mut msg = String::from("未找到匹配的 Memos API 路径，实例版本可能不兼容。\n");
        for (path, detail) in &not_found_paths {
            msg.push_str(&format!("  {path} → {detail}\n"));
        }
        msg.push_str("请确认 Memos 版本为 v0.14 及以上");
        return Err(msg);
    }

    // 全是网络错误
    if let Some(detail) = last_network_error {
        return Err(format!("无法连接 Memos 实例：{detail}"));
    }

    Err("无法访问 Memos 实例".into())
}

fn probe_once(http: &dyn HttpClient, url: &str, token: Option<&str>) -> ProbeResult {
    let request = HttpRequest {
        method: "GET",
        url,
        headers: auth_headers(token),
        body: None,
        timeout_ms: PROBE_TIMEOUT_MS,
    };
    match http.send(request) {
        Ok(response) if response.is_success() => ProbeResult::Success,
        Ok(response) => {
            let status = response.status;
            let body = truncate_body(response.text());
            match status {
                401 | 403 | 405 => ProbeResult::PathExistsAuthIssue(status),
                404 => ProbeResult::NotFound,
                _ => ProbeResult::HttpStatus(status, body),
            }
        }
        Err(error) => ProbeResult::Network(error.to_string()),
    }
}

fn truncate_body(body: String) -> String {
    let trimmed = body.trim();
    if trimmed.len() > 200 {
        format!("{}…", &trimmed[..200])
    } else {
        trimmed.to_string()
    }
}

fn auth_headers(token: Option<&str>) -> Vec<(String, String)> {
    token
        .map(|token| vec![("Authorization".to_string(), format!("Bearer {token}"))])
        .unwrap_or_default()
}
