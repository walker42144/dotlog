use dotlog_domain::{Effect, Instant, PointKind, Segment};

use crate::error::{AppError, AppResult};

/// 计时用例所需的最小快照：当前 active 段与最近一次事实种类。
#[derive(Debug, Clone, Default)]
pub struct TimerSnapshot {
    pub active: Option<Segment>,
    pub last_point_kind: Option<PointKind>,
}

/// 最近一条未被撤销的事实，撤销用例据此重建凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointRecord {
    pub id: String,
    pub kind: PointKind,
    pub occurred_at_ms: i64,
    pub segment_id: Option<String>,
    /// 编辑类事实的撤销载荷（JSON：`{"restore":[...],"remove":[...]}`）。
    pub payload: Option<String>,
}

/// 分类（`category` 表的一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryRow {
    pub id: String,
    pub name: String,
    pub color: String,
    pub parent_id: String,
}

/// outbox 事件状态（`data-dictionary.md` §5.3）。
pub mod outbox_state {
    pub const PENDING: &str = "pending";
    pub const INFLIGHT: &str = "inflight";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
}

/// 外部对象回链状态（`integration_link.state`）。
pub mod link_state {
    pub const PENDING: &str = "pending";
    pub const PUBLISHED: &str = "published";
    pub const FAILED: &str = "failed";
    pub const REMOVED: &str = "removed";
}

/// outbox 队列中的一条事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    pub id: String,
    pub dedupe_key: Option<String>,
    pub event_type: String,
    /// JSON 快照。
    pub payload: String,
    pub state: String,
    pub attempts: i64,
    pub next_retry_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// `integration_link` 表的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationLinkRow {
    pub segment_id: String,
    pub provider: String,
    pub external_id: Option<String>,
    pub state: String,
    pub last_synced_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

/// 启动完整性自检结果（`docs/data-dictionary.md` §9）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntegrityReport {
    pub quick_check_ok: bool,
    pub active_count: i64,
    /// 相邻段重叠的对数（正常应为 0）。
    pub overlap_pairs: i64,
    /// 最早一条 active 段的持续秒数；无 active 为 0。
    pub longest_open_seconds: i64,
}

impl IntegrityReport {
    pub fn ok() -> Self {
        Self {
            quick_check_ok: true,
            active_count: 0,
            overlap_pairs: 0,
            longest_open_seconds: 0,
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.quick_check_ok && self.active_count <= 1 && self.overlap_pairs == 0
    }
}

/// 计时持久化端口。由 `dotlog-storage` 实现，应用层不感知 SQLite。
pub trait TimerStore: Send + Sync {
    fn snapshot(&self) -> AppResult<TimerSnapshot>;

    /// 在单个事务内应用全部变更；任一步失败必须整体回滚。
    fn commit(&self, effects: &[Effect], now_ms: i64) -> AppResult<()>;

    /// Atomically persist link ownership/notes and timer facts. No non-atomic fallback.
    fn commit_runtime(
        &self,
        _effects: &[Effect],
        _key: &str,
        _value: &str,
        _now_ms: i64,
    ) -> AppResult<()> {
        Err(AppError::store("E_LINK_STORE", "存储未实现原子联动提交"))
    }

    fn tomatodo_source_seen(&self, _start_ms: i64) -> AppResult<bool> {
        Ok(false)
    }

    /// 与 `[from_ms, to_ms)` 相交的非删除段，按开始时间升序。
    fn day_segments(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<Segment>>;

    /// 按 id 读取未删除的段。
    fn find_segment(&self, id: &str) -> AppResult<Option<Segment>>;

    /// 最近一条未被撤销的时间点。
    fn last_point(&self) -> AppResult<Option<PointRecord>>;

    fn session_segments(&self, id: &str) -> AppResult<Vec<Segment>> {
        Ok(self.find_segment(id)?.into_iter().collect())
    }

    fn publication_ready(&self, _id: &str) -> AppResult<bool> {
        Ok(true)
    }
    fn confirm_session(&self, _id: &str, _now_ms: i64) -> AppResult<()> {
        Ok(())
    }
    fn pending_note_segment(&self) -> AppResult<Option<Segment>> {
        Ok(None)
    }

    fn current_session_segments(&self) -> AppResult<Vec<Segment>> {
        Ok(self.snapshot()?.active.into_iter().collect())
    }

    /// 历史页查询：按开始时间倒序 + 文本模糊搜索，返回 `(当前页, 总数)`。
    fn history_segments(
        &self,
        from_ms: i64,
        to_ms: i64,
        text: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> AppResult<(Vec<Segment>, u64)>;

    /// 未归档分类，按展示顺序。
    fn categories(&self) -> AppResult<Vec<CategoryRow>> {
        Err(AppError::store("E_UNKNOWN", "未实现分类查询"))
    }

    /// 命名空间设置项：`(key, JSON 值)`。
    fn settings(&self, scope: &str) -> AppResult<Vec<(String, String)>> {
        let _ = scope;
        Err(AppError::store("E_UNKNOWN", "未实现设置读取"))
    }

    /// 写入设置项（单事务）。
    fn settings_upsert(
        &self,
        scope: &str,
        entries: &[(String, String)],
        now_ms: i64,
    ) -> AppResult<()> {
        let _ = (scope, entries, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现设置写入"))
    }

    /// `key = None` 时重置整个命名空间。
    fn settings_reset(&self, scope: &str, key: Option<&str>) -> AppResult<()> {
        let _ = (scope, key);
        Err(AppError::store("E_UNKNOWN", "未实现设置重置"))
    }

    /// 在线快照到指定文件（`VACUUM INTO`）。
    fn backup_to(&self, path: &std::path::Path) -> AppResult<()> {
        let _ = path;
        Err(AppError::store("E_BACKUP_FAILED", "未实现备份"))
    }

    /// 用备份文件覆盖当前库（先做安全副本由调用方负责）。
    fn restore_from(&self, path: &std::path::Path, now_ms: i64) -> AppResult<()> {
        let _ = (path, now_ms);
        Err(AppError::store("E_RESTORE_INVALID", "未实现恢复"))
    }

    /// 读取运行时状态（心跳、退出标记等）。
    fn runtime_state(&self, _key: &str) -> AppResult<Option<String>> {
        Ok(None)
    }

    /// 写入运行时状态（单条 upsert）。
    fn set_runtime_state(&self, _key: &str, _value: &str, _now_ms: i64) -> AppResult<()> {
        Ok(())
    }

    /// 启动完整性自检：`quick_check`、active 数量、重叠与超长未关闭段。
    fn integrity(&self, _now_ms: i64) -> AppResult<IntegrityReport> {
        Ok(IntegrityReport::ok())
    }

    /// 诊断包用的表计数。
    fn table_counts(&self) -> AppResult<Vec<(String, i64)>> {
        Ok(Vec::new())
    }

    /// 当前 schema 版本（`PRAGMA user_version`）。
    fn schema_version(&self) -> AppResult<i64> {
        Ok(0)
    }

    // ---------- outbox（M5 · 可靠异步发送，`docs/plugin-sdk.md` §6）----------

    /// 入队；`dedupe_key` 唯一，重复入队直接忽略并返回 `false`。
    fn outbox_enqueue(
        &self,
        dedupe_key: Option<&str>,
        event_type: &str,
        payload: &str,
        now_ms: i64,
    ) -> AppResult<bool> {
        let _ = (dedupe_key, event_type, payload, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现 outbox 入队"))
    }

    /// 取出到期事件并把它们置为 `inflight`（带 5 分钟租约，超时回收后可再取）。
    fn outbox_take_due(&self, now_ms: i64, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        let _ = (now_ms, limit);
        Ok(Vec::new())
    }

    fn outbox_defer(&self, id: &str, due_ms: i64, now_ms: i64) -> AppResult<()> {
        self.outbox_mark_retry(id, "插件已暂停", due_ms, now_ms)
    }

    fn outbox_mark_done(&self, id: &str, now_ms: i64) -> AppResult<()> {
        let _ = (id, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现 outbox 完成"))
    }

    /// 失败退避：`attempts + 1`，达到上限（6 次）后转 `failed`。
    fn outbox_mark_retry(
        &self,
        id: &str,
        error: &str,
        next_retry_at_ms: i64,
        now_ms: i64,
    ) -> AppResult<()> {
        let _ = (id, error, next_retry_at_ms, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现 outbox 重试"))
    }

    /// 不可恢复失败：直接 `failed`，等待手动重试。
    fn outbox_mark_failed(&self, id: &str, error: &str, now_ms: i64) -> AppResult<()> {
        let _ = (id, error, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现 outbox 失败"))
    }

    /// 集成页队列查询；`state = None` 返回全部，按创建时间倒序。
    fn outbox_list(&self, state: Option<&str>, limit: u32) -> AppResult<Vec<OutboxRecord>> {
        let _ = (state, limit);
        Ok(Vec::new())
    }

    /// 手动重试：回到 `pending`、清零尝试次数、立即到期。
    fn outbox_retry(&self, id: &str, now_ms: i64) -> AppResult<()> {
        let _ = (id, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现 outbox 手动重试"))
    }

    /// 队列概览：`(state, count)`，供集成页徽标。
    fn outbox_counts(&self) -> AppResult<Vec<(String, i64)>> {
        Ok(Vec::new())
    }

    // ---------- integration_link（M5 · 外部对象回链）----------

    /// `tags:write` 能力：为段附加标签（不存在则创建），并写 `edit` 事实便于撤销。
    fn segment_attach_tag(&self, segment_id: &str, tag: &str, now_ms: i64) -> AppResult<()> {
        let _ = (segment_id, tag, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现标签写入"))
    }

    fn integration_link_get(
        &self,
        segment_id: &str,
        provider: &str,
    ) -> AppResult<Option<IntegrationLinkRow>> {
        let _ = (segment_id, provider);
        Ok(None)
    }

    fn integration_link_upsert(
        &self,
        segment_id: &str,
        provider: &str,
        external_id: Option<&str>,
        state: &str,
        error: Option<&str>,
        now_ms: i64,
    ) -> AppResult<()> {
        let _ = (segment_id, provider, external_id, state, error, now_ms);
        Err(AppError::store("E_UNKNOWN", "未实现回链写入"))
    }

    /// 某提供方最近一次成功同步时间（集成页 `lastSyncAt`）。
    fn integration_last_sync(&self, provider: &str) -> AppResult<Option<i64>> {
        let _ = provider;
        Ok(None)
    }
}

/// 时间来源。实现放在外层（桌面端取系统本地时间），便于测试注入固定时钟。
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}
