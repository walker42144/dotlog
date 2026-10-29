//! 分类目录、备份与恢复用例。
//!
//! 备份文件的命名、保留策略与目录列举属于桌面层；应用层只负责目录读取与
//! 端口的转发，保证 UI 消费的契约固定。

use std::path::Path;

use crate::dto::CategoryView;
use crate::error::AppResult;
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// `categories_list`：未归档分类，按展示顺序。
    pub fn categories(&self) -> AppResult<Vec<CategoryView>> {
        Ok(self
            .store
            .categories()?
            .into_iter()
            .map(|row| CategoryView {
                id: row.id,
                name: row.name,
                color: row.color,
                parent_id: row.parent_id,
            })
            .collect())
    }

    /// `backup_now` 的核心：在线快照到给定文件。
    pub fn backup_to(&self, path: &Path) -> AppResult<()> {
        self.store.backup_to(path)
    }

    /// `backup_restore` 的核心：用备份覆盖当前库（成功后由 UI 重新拉取状态）。
    pub fn restore_from(&self, path: &Path) -> AppResult<()> {
        let _operation = self.operation.lock().unwrap_or_else(|e| e.into_inner());
        self.store.restore_from(path, self.clock.now().utc_ms)
    }
}
