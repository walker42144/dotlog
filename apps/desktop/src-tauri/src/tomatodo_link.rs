//! Host-owned runtime. UI plugins have scoped commands, never arbitrary CDP/SQL access.
use crate::tomatodo_observer::NativeObserver;
use crate::{ensure_ready, store_error, AppState, IpcError, IpcResult};
use dotlog_application::dto::{PunchResultView, SegmentView};
use dotlog_application::tomatodo_link::{LinkCommand, Observation, SourcePhase};
use serde_json::{json, Value};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const RETRY_INTERVAL: Duration = Duration::from_secs(5);

fn retry_due(last: &mut Option<Instant>, now: Instant) -> bool {
    if last.is_some_and(|at| now.duration_since(at) < RETRY_INTERVAL) {
        return false;
    }
    *last = Some(now);
    true
}

#[derive(Default)]
pub struct Runtime {
    pub(super) gate: Mutex<()>,
    owner: Mutex<Option<String>>,
    error: Mutex<Option<String>>,
    observer: Mutex<NativeObserver>,
}
fn fail(message: impl Into<String>) -> IpcError {
    store_error("E_TOMATODO_LINK", message.into())
}
pub fn enabled(state: &AppState) -> bool {
    state
        .link
        .owner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

pub fn start(state: &AppState, app: &AppHandle, id: &str) -> IpcResult<()> {
    ensure_ready(state)?;
    let _guard = state.link.gate.lock().map_err(|_| fail("后台锁不可用"))?;
    if !super::local_plugins::link_authorized(state, id)? {
        return Err(fail("插件未启用或未授权联动"));
    }
    let owner = state
        .link
        .owner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if owner.as_deref().is_some_and(|owner| owner != id) {
        return Err(fail("联动已由另一个插件启用"));
    }
    *state.link.owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_owned());
    *state.link.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let _ = app.emit("dotlog://link-changed", ());
    Ok(())
}

fn ensure_source_started() -> IpcResult<()> {
    let endpoint = std::net::SocketAddr::from(([127, 0, 0, 1], 19275));
    if std::net::TcpStream::connect_timeout(&endpoint, Duration::from_millis(200)).is_ok() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let system = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:/Windows".into());
        let output = Command::new(system.join("System32/tasklist.exe"))
            .args(["/FI", "IMAGENAME eq TomaToDo.exe", "/FO", "CSV", "/NH"])
            .creation_flags(0x08000000)
            .output()
            .map_err(|e| fail(format!("无法检查番茄是否正在运行：{e}")))?;
        if !output.status.success() {
            return Err(fail("无法确认番茄运行状态，未自动启动第二个实例"));
        }
        if String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("tomatodo.exe")
        {
            return Err(fail("番茄已运行但没有开启联动连接。请在专注结束后正常退出番茄；DotLog 会自动重新打开并连接，不必运行脚本"));
        }
        let source = std::env::var_os("ProgramFiles")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:/Program Files".into())
            .join("TomaToDo/TomaToDo.exe");
        if !source.is_file() {
            return Err(fail("未找到已安装的官方番茄 ToDo"));
        }
        Command::new(source)
            .args([
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=19275",
            ])
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|e| fail(format!("自动打开番茄失败：{e}")))?;
        Ok(())
    }
    #[cfg(not(windows))]
    Err(fail("当前仅支持自动打开 Windows 番茄客户端"))
}

pub fn stop(state: &AppState, owner: &str) -> IpcResult<()> {
    let _guard = state.link.gate.lock().map_err(|_| fail("后台锁不可用"))?;
    if state
        .link
        .owner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        != Some(owner)
    {
        return Ok(());
    }
    state
        .timer
        .tomatodo_link(LinkCommand::Detach)
        .map_err(IpcError::from)?;
    *state.link.owner.lock().unwrap_or_else(|e| e.into_inner()) = None;
    state
        .link
        .observer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reset();
    Ok(())
}

pub fn status(state: &AppState) -> IpcResult<Value> {
    let context = state.timer.tomatodo_link_state().map_err(IpcError::from)?;
    let notes = context
        .pending_notes
        .iter()
        .map(|id| {
            state.store.find_segment(id).map(|s| {
                Some(s.map(|s| {
                    json!({"id":s.id,"title":s.title,"note":s.note,"revision":s.revision,
            "startAt":s.start_at_ms,"endAt":s.end_at_ms})
                }).unwrap_or_else(||json!({"id":id,"title":"记录已删除或不可用","note":"","revision":0,"missing":true})))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(IpcError::from)?;
    Ok(
        json!({"enabled":enabled(state),"owner":state.link.owner.lock().unwrap_or_else(|e|e.into_inner()).clone(),
        "error":state.link.error.lock().unwrap_or_else(|e|e.into_inner()).clone(),"context":context,
        "pendingNotes":notes.into_iter().flatten().collect::<Vec<_>>(),
        "now":chrono::Local::now().timestamp_millis()}),
    )
}

pub fn rpc(
    state: &AppState,
    app: &AppHandle,
    id: &str,
    action: &str,
    params: &Value,
) -> IpcResult<Value> {
    if action == "status" {
        return status(state);
    }
    ensure_ready(state)?;
    if action == "disable" {
        stop(state, id)?;
        super::local_plugins::set_auto_link(state, id, false)?;
        let _ = app.emit("dotlog://link-changed", ());
        return status(state);
    }
    if action == "enable" {
        super::local_plugins::set_auto_link(state, id, true)?;
        start(state, app, id)?;
    } else {
        let _guard = state.link.gate.lock().map_err(|_| fail("后台锁不可用"))?;
        let command = match action {
            "startNext" => LinkCommand::StartNext,
            "endSegment" => LinkCommand::EndSegment,
            "reconcile" => LinkCommand::Reconcile {
                end_at_ms: params
                    .get("endAt")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| fail("请选择核对截止时间"))?,
            },
            "saveNote" | "skipNote" => {
                let segment_id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| fail("缺少段ID"))?
                    .to_owned();
                let revision = params
                    .get("revision")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| fail("缺少记录版本"))?;
                if action == "skipNote" {
                    LinkCommand::SkipNote {
                        segment_id,
                        revision,
                    }
                } else {
                    LinkCommand::SaveNote {
                        segment_id,
                        revision,
                        note: params
                            .get("note")
                            .and_then(Value::as_str)
                            .ok_or_else(|| fail("缺少备注"))?
                            .into(),
                    }
                }
            }
            _ => return Err(fail("不支持的联动操作")),
        };
        if matches!(
            command,
            LinkCommand::StartNext | LinkCommand::EndSegment | LinkCommand::Reconcile { .. }
        ) && state
            .link
            .owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            != Some(id)
        {
            return Err(fail("请先由本插件启用联动"));
        }
        state.timer.tomatodo_link(command).map_err(IpcError::from)?;
    }
    let _ = app.emit("dotlog://link-changed", ());
    status(state)
}

pub fn manual_end(state: &AppState) -> IpcResult<PunchResultView> {
    let _guard = state.link.gate.lock().map_err(|_| fail("后台锁不可用"))?;
    let id = state
        .timer
        .tomatodo_link_state()
        .map_err(IpcError::from)?
        .active_segment_id
        .ok_or_else(|| fail("当前没有联动分段"))?;
    state
        .timer
        .tomatodo_link(LinkCommand::EndSegment)
        .map_err(IpcError::from)?;
    let segment = state
        .store
        .find_segment(&id)
        .map_err(IpcError::from)?
        .ok_or_else(|| fail("记录不存在"))?;
    Ok(PunchResultView {
        closed_segment: SegmentView::from_segment(
            &segment,
            chrono::Local::now().timestamp_millis(),
        ),
        new_segment: None,
        pending_note: None,
        undo_token: None,
        short_segment: false,
    })
}
pub fn manual_start(state: &AppState) -> IpcResult<()> {
    let _guard = state.link.gate.lock().map_err(|_| fail("后台锁不可用"))?;
    state
        .timer
        .tomatodo_link(LinkCommand::StartNext)
        .map_err(IpcError::from)?;
    Ok(())
}

pub fn spawn_worker(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last_source: Option<(SourcePhase, Option<String>)> = None;
        let mut integrity_ok = false;
        let mut last_restore_check = None;
        let mut last_source_check = None;
        let mut source_issue: Option<String> = None;
        loop {
            std::thread::sleep(Duration::from_millis(750));
            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            if !enabled(&state) {
                last_source = None;
                integrity_ok = false;
                state
                    .link
                    .observer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .reset();
                last_source_check = None;
                source_issue = None;
                if retry_due(&mut last_restore_check, Instant::now()) {
                    if let Err(error) = super::local_plugins::restore_auto_link(&state, &app) {
                        *state.link.error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(error.message);
                    }
                }
                continue;
            }
            last_restore_check = None;
            let Ok(_guard) = state.link.gate.lock() else {
                continue;
            };
            let owner = state
                .link
                .owner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let Some(owner) = owner else {
                continue;
            };
            if !super::local_plugins::link_authorized(&state, &owner).unwrap_or(false) {
                continue;
            }
            if state
                .recovery
                .lock()
                .map(|cache| cache.info.required)
                .unwrap_or(true)
            {
                *state.link.error.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some("请先处理计时恢复提示".into());
                continue;
            }
            let sample = {
                let mut observer = state
                    .link
                    .observer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                match observer.snapshot() {
                    Ok(sample) => {
                        source_issue = None;
                        last_source_check = None;
                        Ok(sample)
                    }
                    Err(message) => {
                        if retry_due(&mut last_source_check, Instant::now()) {
                            source_issue = ensure_source_started().err().map(|e| e.message);
                        }
                        Err(source_issue.as_ref().unwrap_or(&message).clone())
                    }
                }
            };
            let before = state.timer.tomatodo_link_state().ok();
            let result = match sample {
                Ok(sample) => {
                    let fingerprint =
                        (sample.phase, sample.session.as_ref().map(|s| s.key.clone()));
                    let transition = last_source.as_ref() != Some(&fingerprint);
                    last_source = Some(fingerprint);
                    if transition || !integrity_ok {
                        if let Err(error) = ensure_ready(&state) {
                            integrity_ok = false;
                            Err(error.message)
                        } else {
                            integrity_ok = true;
                            state
                                .timer
                                .tomatodo_link(LinkCommand::ObserveAutomatic(sample))
                                .map_err(|e| e.to_string())
                        }
                    } else {
                        state
                            .timer
                            .tomatodo_link(LinkCommand::ObserveAutomatic(sample))
                            .map_err(|e| e.to_string())
                    }
                }
                Err(message) => {
                    last_source = None;
                    let _ = state.timer.tomatodo_link(LinkCommand::Observe(Observation {
                        phase: SourcePhase::Unknown,
                        session: None,
                        observed_at_ms: chrono::Local::now().timestamp_millis(),
                        count_up: false,
                    }));
                    Err(format!(
                        "连接读取失败：{message}。DotLog 内置观察器会自动重连"
                    ))
                }
            };
            match result {
                Ok(context) => {
                    *state.link.error.lock().unwrap_or_else(|e| e.into_inner()) =
                        if context.connected {
                            None
                        } else {
                            Some("正在自动打开或连接番茄 ToDo，请稍候".into())
                        };
                    if before.as_ref().map_or(true, |b| {
                        b.active_segment_id != context.active_segment_id
                            || b.pending_notes != context.pending_notes
                            || b.source_phase != context.source_phase
                            || b.conflict != context.conflict
                    }) {
                        let _ = app.emit("dotlog://link-changed", ());
                    }
                }
                Err(message) => {
                    *state.link.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message)
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_schedule_is_monotonic_and_throttled() {
        let now = Instant::now();
        let mut last = None;
        assert!(retry_due(&mut last, now));
        assert!(!retry_due(&mut last, now + Duration::from_secs(4)));
        assert!(retry_due(&mut last, now + Duration::from_secs(5)));
        assert!(!retry_due(&mut last, now + Duration::from_secs(6)));
    }
}
