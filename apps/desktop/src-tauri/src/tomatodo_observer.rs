//! In-process, read-only CDP observation. No helper executable, script file or HTTP server.
use dotlog_application::tomatodo_link::{Observation, SourcePhase, SourceSession};
use serde_json::{json, Value};
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use tungstenite::{client::client_with_config, protocol::WebSocketConfig, Message, WebSocket};
use url::Url;

const LIMIT: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_millis(2000);
// Embedded scalar reads only. CDP rejects side effects; no source IPC or storage operations.
const EXPRESSION: &str = r#"(() => {
 if (typeof TimerCoreState !== 'object' || TimerCoreState === null ||
     typeof TimerCoreState.isRunning !== 'boolean' || typeof isRestMode !== 'boolean')
   return { compatible: false };
 const c = TimerCoreState;
 const t = typeof currentTodo === 'object' && currentTodo !== null ? currentTodo : c.targetTodo;
 return { compatible: true, running: c.isRunning, paused: c.isPauseTimer === true ||
   (typeof isPaused === 'boolean' && isPaused), rest: isRestMode,
   taskId: t && (typeof t.id === 'number' || typeof t.id === 'string') ? t.id : null,
   title: t && typeof t.name === 'string' ? t.name : '',
   startMs: c.startDate,
   countUp: typeof isCountUp === 'boolean' ? isCountUp : null };
})()"#;

pub(super) struct NativeObserver {
    port: u16,
    agent: ureq::Agent,
    socket: Option<WebSocket<TcpStream>>,
    next_id: u64,
}
impl Default for NativeObserver {
    fn default() -> Self {
        Self::at_port(19275)
    }
}
impl NativeObserver {
    fn at_port(port: u16) -> Self {
        Self {
            port,
            agent: ureq::AgentBuilder::new()
                .timeout(TIMEOUT)
                .redirects(0)
                .build(),
            socket: None,
            next_id: 0,
        }
    }
    pub(super) fn reset(&mut self) {
        self.socket = None;
    }
    pub(super) fn snapshot(&mut self) -> Result<Observation, String> {
        let result = self.read_snapshot();
        if result.is_err() {
            self.reset();
        }
        result
    }
    fn connect(&mut self) -> Result<(), String> {
        let response = self
            .agent
            .get(&format!("http://127.0.0.1:{}/json/list", self.port))
            .call()
            .map_err(|e| format!("无法读取官方番茄连接：{e}"))?;
        let mut body = String::new();
        response
            .into_reader()
            .take((LIMIT + 1) as u64)
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        if body.len() > LIMIT {
            return Err("番茄页面列表超过限制".into());
        }
        let targets: Value = serde_json::from_str(&body).map_err(|_| "番茄页面列表格式错误")?;
        let endpoint = select_target(&targets, self.port)?;
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        let stream = TcpStream::connect_timeout(&address, TIMEOUT).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|e| e.to_string())?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(LIMIT))
            .max_frame_size(Some(LIMIT));
        let (socket, _) = client_with_config(endpoint.as_str(), stream, Some(config))
            .map_err(|e| format!("番茄本机连接握手失败：{e}"))?;
        self.socket = Some(socket);
        Ok(())
    }
    fn read_snapshot(&mut self) -> Result<Observation, String> {
        if self.socket.is_none() {
            self.connect()?;
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("连接序号耗尽，请重新打开 DotLog")?;
        let id = self.next_id;
        let socket = self.socket.as_mut().ok_or("番茄尚未连接")?;
        socket.send(Message::Text(json!({"id":id,"method":"Runtime.evaluate","params":{
            "expression":EXPRESSION,"returnByValue":true,"throwOnSideEffect":true,"timeout":1000
        }}).to_string().into())).map_err(|e|e.to_string())?;
        let deadline = Instant::now() + TIMEOUT;
        for _ in 0..32 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("读取番茄状态超时".into());
            }
            socket
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(|e| e.to_string())?;
            let message = socket.read().map_err(|e| e.to_string())?;
            if let Message::Text(text) = message {
                let v: Value =
                    serde_json::from_str(text.as_str()).map_err(|_| "番茄响应格式错误")?;
                if v.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if v.get("error").is_some() || v.pointer("/result/exceptionDetails").is_some() {
                    return Err("番茄只读状态读取失败，已停止解释计时事件".into());
                }
                return normalize(
                    v.pointer("/result/result/value")
                        .ok_or("番茄响应缺少状态")?,
                    chrono::Local::now().timestamp_millis(),
                );
            }
        }
        Err("番茄连接响应数量异常".into())
    }
}

fn official_page(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "file"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Ok(path) = url.to_file_path() else {
        return false;
    };
    let expected = std::env::var_os("ProgramFiles")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:/Program Files".into())
        .join("TomaToDo/resources/app.asar/index.html");
    path.to_string_lossy()
        .replace('\\', "/")
        .eq_ignore_ascii_case(&expected.to_string_lossy().replace('\\', "/"))
}
fn select_target(targets: &Value, port: u16) -> Result<Url, String> {
    let targets = targets.as_array().ok_or("番茄页面列表格式错误")?;
    let matches: Vec<_> = targets
        .iter()
        .filter(|t| {
            t.get("type").and_then(Value::as_str) == Some("page")
                && t.get("url")
                    .and_then(Value::as_str)
                    .is_some_and(official_page)
        })
        .collect();
    if matches.len() != 1 {
        return Err("未找到唯一的官方番茄页面，请检查版本与安装位置".into());
    }
    let endpoint = Url::parse(
        matches[0]
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or("番茄缺少本机连接地址")?,
    )
    .map_err(|_| "番茄连接地址格式错误")?;
    if endpoint.scheme() != "ws"
        || endpoint.host_str() != Some("127.0.0.1")
        || endpoint.port() != Some(port)
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !endpoint.path().starts_with("/devtools/page/")
        || endpoint.path().len() <= 15
    {
        return Err("拒绝非预期的本机番茄连接地址".into());
    }
    Ok(endpoint)
}
fn normalize(raw: &Value, now: i64) -> Result<Observation, String> {
    if raw.get("compatible").and_then(Value::as_bool) != Some(true) {
        return Err("当前番茄版本未提供预期状态".into());
    }
    let running = raw
        .get("running")
        .and_then(Value::as_bool)
        .ok_or("缺少番茄运行状态")?;
    let rest = raw
        .get("rest")
        .and_then(Value::as_bool)
        .ok_or("缺少番茄休息状态")?;
    let paused = raw
        .get("paused")
        .and_then(Value::as_bool)
        .ok_or("缺少番茄暂停状态")?;
    let phase = if rest {
        SourcePhase::Rest
    } else if !running {
        SourcePhase::Idle
    } else if paused {
        SourcePhase::Paused
    } else {
        SourcePhase::Focus
    };
    let count_up = raw.get("countUp").and_then(Value::as_bool) == Some(true);
    let session = if matches!(phase, SourcePhase::Focus | SourcePhase::Paused) {
        let started = raw
            .get("startMs")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0 && *n <= now)
            .ok_or("缺少有效的番茄开始时间")?;
        let title = raw
            .get("title")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or("缺少番茄任务标题")?;
        let task = raw.get("taskId").ok_or("缺少番茄任务身份")?;
        let task = if let Some(s) = task.as_str() {
            s.to_owned()
        } else if task.is_i64() || task.is_u64() {
            task.to_string()
        } else {
            return Err("番茄任务身份格式错误".into());
        };
        if !count_up || task.is_empty() || task.len() > 480 {
            return Err("番茄身份不完整或不是正计时".into());
        }
        Some(SourceSession {
            key: format!("{task}:{started}"),
            title: title.chars().take(500).collect(),
            started_at_ms: started,
        })
    } else {
        None
    };
    Ok(Observation {
        phase,
        session,
        observed_at_ms: now,
        count_up,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    fn raw() -> Value {
        json!({"compatible":true,"running":true,"rest":false,"paused":false,"countUp":true,"taskId":2263,"title":"线代","startMs":1000})
    }
    fn target(port: u16) -> Value {
        json!([{"type":"page","url":"file:///C:/Program%20Files/TomaToDo/resources/app.asar/index.html","webSocketDebuggerUrl":format!("ws://127.0.0.1:{port}/devtools/page/test")}])
    }
    #[test]
    fn raw_focus_pause_rest_and_idle_have_the_expected_identity() {
        let mut v = raw();
        let first = normalize(&v, 2000).unwrap();
        assert_eq!(first.phase, SourcePhase::Focus);
        assert_eq!(first.session.unwrap().key, "2263:1000");
        v["paused"] = json!(true);
        assert_eq!(normalize(&v, 2000).unwrap().phase, SourcePhase::Paused);
        v["rest"] = json!(true);
        assert_eq!(normalize(&v, 2000).unwrap().phase, SourcePhase::Rest);
        v["running"] = json!(false);
        v["rest"] = json!(false);
        assert_eq!(normalize(&v, 2000).unwrap().phase, SourcePhase::Idle);
    }
    #[test]
    fn invalid_samples_never_become_idle_or_a_fabricated_focus() {
        assert!(normalize(&json!({}), 2000).is_err());
        for field in [
            "running", "rest", "paused", "startMs", "taskId", "title", "countUp",
        ] {
            let mut v = raw();
            v.as_object_mut().unwrap().remove(field);
            assert!(normalize(&v, 2000).is_err(), "{field}");
        }
        let mut v = raw();
        v["startMs"] = json!(3000);
        assert!(normalize(&v, 2000).is_err());
    }
    #[test]
    fn endpoint_is_bound_to_the_unique_official_page_and_local_port() {
        assert!(select_target(&target(19275), 19275).is_ok());
        for endpoint in [
            "ws://example.com:19275/devtools/page/test",
            "ws://127.0.0.1:1/devtools/page/test",
            "wss://127.0.0.1:19275/devtools/page/test",
            "ws://name@127.0.0.1:19275/devtools/page/test",
            "ws://127.0.0.1:19275/other",
        ] {
            let mut v = target(19275);
            v[0]["webSocketDebuggerUrl"] = json!(endpoint);
            assert!(select_target(&v, 19275).is_err());
        }
        let item = target(19275)[0].clone();
        assert!(select_target(&json!([item.clone(), item]), 19275).is_err());
        let mut v = target(19275);
        v[0]["url"] = json!("file:///C:/unrelated/index.html");
        assert!(select_target(&v, 19275).is_err());
    }
    fn serve_round(listener: &TcpListener, values: &[Value]) {
        let port = listener.local_addr().unwrap().port();
        let (mut http, _) = listener.accept().unwrap();
        http.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut request = [0; 4096];
        let n = http.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /json/list "));
        let body = target(port).to_string();
        write!(
            http,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        drop(http);
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        for value in values {
            let request: Value =
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            assert_eq!(request["params"]["throwOnSideEffect"], true);
            assert_eq!(request["params"]["expression"], EXPRESSION);
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":{"result":{"value":value}}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
        }
        socket.close(None).unwrap();
    }
    #[test]
    fn native_socket_reads_reuses_and_reconnects_without_any_helper_process() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let mut rest = raw();
            rest["rest"] = json!(true);
            serve_round(&listener, &[raw(), rest]);
            serve_round(&listener, &[raw()]);
        });
        let mut observer = NativeObserver::at_port(port);
        assert_eq!(observer.snapshot().unwrap().phase, SourcePhase::Focus);
        assert_eq!(observer.snapshot().unwrap().phase, SourcePhase::Rest);
        assert!(observer.snapshot().is_err());
        assert!(observer.socket.is_none());
        assert_eq!(observer.snapshot().unwrap().phase, SourcePhase::Focus);
        observer.reset();
        thread.join().unwrap();
    }
}
