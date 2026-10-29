//! 启动核对、恢复处理与诊断快照（`docs/roadmap.md` M4）。
//!
//! 检测规则在 `dotlog-domain::recovery`；本模块只负责读取运行时状态、
//! 组装事务与把领域结果映射成视图。

use dotlog_domain::recovery::{
    detect, resolve, RecoveryAction, RecoveryContext, RecoveryReason, RecoveryResolution,
};
use dotlog_domain::day_window;

use crate::dto::{
    DiagnosticsView, IntegrityView, RecoveryInfoView, StartupReport, TableCountView,
    TimerStateView,
};
use crate::error::AppResult;
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;

/// 最后一次心跳的 wall 时间（epoch 毫秒），用于检测休眠与长间隔。
pub const HEARTBEAT_KEY: &str = "heartbeat.wall_ms";
/// 上次进程是否正常退出；崩溃或强杀时保持 `"false"`。
pub const CLEAN_SHUTDOWN_KEY: &str = "lifecycle.clean_shutdown";

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// 启动核对：读取上一次会话的心跳与退出标记，检测异常并开启新会话。
    ///
    /// 必须在窗口展示前调用一次；返回的 `recovery` 由桌面层缓存，
    /// 因为新会话会立即覆盖心跳，之后无法再还原检测结果。
    pub fn begin_session(&self) -> AppResult<StartupReport> {
        let now = self.clock.now();
        let policy = self.day_policy()?;
        let (day_start, _) = day_window(now.utc_ms, now.tz_offset_min, policy.day_start_minutes);

        let heartbeat_ms = self
            .store
            .runtime_state(HEARTBEAT_KEY)?
            .and_then(|raw| raw.parse::<i64>().ok());
        let clean_shutdown = self
            .store
            .runtime_state(CLEAN_SHUTDOWN_KEY)?
            .map_or(true, |raw| raw == "true");
        let integrity = self.store.integrity(now.utc_ms)?;
        let state = self.load_state()?;

        let check = detect(
            state.active.as_ref(),
            &RecoveryContext {
                now_ms: now.utc_ms,
                day_window_start_ms: day_start,
                last_heartbeat_ms: heartbeat_ms,
                clean_shutdown,
                ..RecoveryContext::default()
            },
        );

        // 从现在起本进程「未正常退出」，直到 `end_session` 或恢复处理完成。
        self.store
            .set_runtime_state(CLEAN_SHUTDOWN_KEY, "false", now.utc_ms)?;
        self.store
            .set_runtime_state(HEARTBEAT_KEY, &now.utc_ms.to_string(), now.utc_ms)?;

        Ok(StartupReport {
            recovery: RecoveryInfoView {
                required: check.required,
                reason: check.reason,
                active_segment: state
                    .active
                    .as_ref()
                    .map(|segment| self.view_of(segment, now.utc_ms)),
                detected_at: now.utc_ms,
                options: if check.required {
                    check.options()
                } else {
                    Vec::new()
                },
            },
            integrity: IntegrityView::from_report(integrity),
            state: self.view(&state),
            cutoff_ms: check.cutoff_ms,
        })
    }

    /// `recovery_resolve` 的核心：按用户选择关闭、截断或丢弃 active 段。
    ///
    /// 处理完成后标记为干净会话，避免下一次启动重复提示。
    pub fn resolve_recovery(
        &self,
        reason: RecoveryReason,
        action: RecoveryAction,
        cutoff_ms: Option<i64>,
    ) -> AppResult<TimerStateView> {
        let at = self.clock.now();
        let state = self.load_state()?;
        let transition = resolve(
            &state,
            &RecoveryResolution {
                reason,
                action,
                at,
                cutoff_ms,
                point_id: crate::new_id(),
            },
        )?;

        if !transition.effects.is_empty() {
            self.store.commit(&transition.effects, at.utc_ms)?;
        }
        self.store
            .set_runtime_state(CLEAN_SHUTDOWN_KEY, "true", at.utc_ms)?;
        self.store
            .set_runtime_state(HEARTBEAT_KEY, &at.utc_ms.to_string(), at.utc_ms)?;

        Ok(self.view(&transition.state))
    }

    /// 定期心跳（桌面端每 30 秒调用一次）。
    pub fn record_heartbeat(&self) -> AppResult<()> {
        let now_ms = self.clock.now().utc_ms;
        self.store
            .set_runtime_state(HEARTBEAT_KEY, &now_ms.to_string(), now_ms)
    }

    /// 正常退出：写干净标记与最后一次心跳。
    pub fn end_session(&self) -> AppResult<()> {
        let now_ms = self.clock.now().utc_ms;
        self.store
            .set_runtime_state(CLEAN_SHUTDOWN_KEY, "true", now_ms)?;
        self.store
            .set_runtime_state(HEARTBEAT_KEY, &now_ms.to_string(), now_ms)
    }

    /// 诊断快照的数据部分：schema 版本、表计数与完整性自检。
    pub fn diagnostics(&self) -> AppResult<DiagnosticsView> {
        let now_ms = self.clock.now().utc_ms;
        let integrity = self.store.integrity(now_ms)?;
        Ok(DiagnosticsView {
            schema_version: self.store.schema_version()?,
            generated_at: now_ms,
            tables: self
                .store
                .table_counts()?
                .into_iter()
                .map(|(name, count)| TableCountView { name, count })
                .collect(),
            integrity: IntegrityView::from_report(integrity),
        })
    }
}
