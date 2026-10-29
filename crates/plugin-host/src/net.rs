//! HTTP 与密钥存储的 host 实现。

use std::io::Read;
use std::time::Duration;

use dotlog_plugin_sdk::{HttpClient, HttpRequest, HttpResponse, PluginError, SecretStore};

/// 从 URL 提取主机名（小写）；支持 userinfo 与端口，兼容 IPv6 括号。
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next()?.to_string()
    } else {
        host.split(':').next()?.to_string()
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// 域名白名单 HTTP 客户端（`network` 能力）。白名单非空时，
/// 目标主机不在名单内直接拒绝，不建立连接（§3）。
pub struct UreqClient {
    pub allowed_hosts: Vec<String>,
    /// 单请求超时上限（毫秒）；插件可要求更短。
    pub timeout_ms: u64,
}

impl HttpClient for UreqClient {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        let host = host_of(request.url)
            .ok_or_else(|| PluginError::Network(format!("URL 无法解析：{}", request.url)))?;
        if !self.allowed_hosts.is_empty()
            && !self
                .allowed_hosts
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&host))
        {
            return Err(PluginError::Network(format!(
                "域名 `{host}` 不在插件声明的联网白名单内"
            )));
        }

        let timeout = request.timeout_ms.min(self.timeout_ms).max(1);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(timeout))
            .build();
        let method = request.method.to_ascii_uppercase();
        let mut req = agent.request(&method, request.url);
        for (name, value) in &request.headers {
            req = req.set(name, value);
        }

        // 4xx/5xx 也要拿到状态码与响应体（handler 据此区分 RetryLater / Fail）。
        let response = match request.body {
            Some(bytes) => req.send_bytes(&bytes),
            None => req.call(),
        };
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(PluginError::Network(error.to_string())),
        };

        let status = response.status();
        let mut body = Vec::new();
        response
            .into_reader()
            .take(4 * 1024 * 1024)
            .read_to_end(&mut body)
            .map_err(|error| PluginError::Network(format!("读取响应失败：{error}")))?;
        Ok(HttpResponse { status, body })
    }
}

/// 系统钥匙串实现；命名空间 `dotlog.<pluginId>`，键名即服务内条目（§3）。
pub struct KeyringStore {
    pub namespace: String,
}

impl KeyringStore {
    pub fn for_plugin(plugin_id: &str) -> Self {
        Self {
            namespace: format!("dotlog.{plugin_id}"),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, PluginError> {
        keyring::Entry::new(&self.namespace, key)
            .map_err(|error| PluginError::Other(format!("钥匙串不可用：{error}")))
    }
}

impl SecretStore for KeyringStore {
    fn get_secret(&self, key: &str) -> Result<Option<String>, PluginError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(PluginError::Other(format!("钥匙串读取失败：{error}"))),
        }
    }

    fn set_secret(&self, key: &str, value: &str) -> Result<(), PluginError> {
        // Windows Credential Manager 的 CredWrite 语义为「存在即覆盖」。
        self.entry(key)?
            .set_password(value)
            .map_err(|error| PluginError::Other(format!("钥匙串写入失败：{error}")))
    }

    fn delete_secret(&self, key: &str) -> Result<(), PluginError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(PluginError::Other(format!("钥匙串删除失败：{error}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_parses_userinfo_port_and_case() {
        assert_eq!(host_of("https://memos.example.com/api/v1/status").as_deref(), Some("memos.example.com"));
        assert_eq!(host_of("http://user:pass@192.168.1.5:5230/memos").as_deref(), Some("192.168.1.5"));
        assert_eq!(host_of("HTTPS://Example.COM:443/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://[::1]:5230/").as_deref(), Some("::1"));
        assert_eq!(host_of("not-a-url"), None);
        assert_eq!(host_of("https:///path"), None);
    }

    #[test]
    fn ureq_client_rejects_hosts_outside_allowlist() {
        let client = UreqClient {
            allowed_hosts: vec!["memos.local".into()],
            timeout_ms: 1_000,
        };
        let request = HttpRequest {
            method: "GET",
            url: "https://example.com/api",
            headers: Vec::new(),
            body: None,
            timeout_ms: 1_000,
        };
        let error = client.send(request).unwrap_err();
        assert!(error.to_string().contains("白名单"), "{error}");
    }
}
