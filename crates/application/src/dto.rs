use dotlog_domain::{Phase, PunchMode, RecoveryAction, RecoveryReason, Segment};
use serde::{Deserialize, Serialize};

/// 与 `docs/ipc-contract.md` §2 对齐的视图类型；字段名自动转为 camelCase。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentView {
    pub id: String,
    pub start_at: i64,
    pub end_at: Option<i64>,
    pub duration_seconds: i64,
    pub title: String,
    pub note: Option<String>,
    pub category: Option<CategoryView>,
    pub tags: Vec<TagView>,
    pub is_focus: bool,
    pub is_ignored: bool,
    pub status: &'static str,
    pub source: &'static str,
    pub revision: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub id: String,
    pub name: String,
    pub color: String,
    pub parent_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagView {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

impl SegmentView {
    pub fn from_segment(segment: &Segment, now_ms: i64) -> Self {
        Self {
            id: segment.id.clone(),
            start_at: segment.start_at_ms,
            end_at: segment.end_at_ms,
            duration_seconds: segment.duration_ms(now_ms) / 1_000,
            title: segment.title.clone(),
            note: if segment.note.is_empty() {
                None
            } else {
                Some(segment.note.clone())
            },
            // 分类由调用方按 id 关联（`with_category`）；标签在 M4 接入。
            category: None,
            tags: Vec::new(),
            is_focus: segment.is_focus,
            is_ignored: segment.is_ignored,
            status: if segment.is_active() { "active" } else { "closed" },
            source: "manual",
            revision: segment.revision,
        }
    }

    /// 附加分类视图；`category_id` 为空时保持 `None`。
    pub fn with_category(mut self, category: Option<CategoryView>) -> Self {
        self.category = category;
        self
    }

    /// 跨日视图用的切片：起止取窗口交集，`id` 保留原始段 id 以便跳转编辑。
    pub fn from_slice(segment: &Segment, start_ms: i64, end_ms: i64, now_ms: i64) -> Self {
        let mut view = Self::from_segment(segment, now_ms);
        view.start_at = start_ms;
        view.duration_seconds = (end_ms - start_ms) / 1_000;
        if !segment.is_active() {
            view.end_at = Some(end_ms);
        }
        view
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerStateView {
    pub phase: Phase,
    pub active: Option<SegmentView>,
    pub elapsed_seconds: i64,
    pub mode: PunchMode,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingNoteView {
    pub segment_id: String,
    pub start_at: i64,
    pub end_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PunchResultView {
    pub closed_segment: SegmentView,
    pub new_segment: Option<SegmentView>,
    pub pending_note: Option<PendingNoteView>,
    /// 15 秒撤销窗口的凭据；`timer_undo` 可省略（后端以最近事实为准）。
    pub undo_token: Option<String>,
    /// 关闭段短于 15 秒，UI 提示补记。
    pub short_segment: bool,
}

/// `timer_undo` 结果：撤销打点会重开段，撤销开始会删除空段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResultView {
    pub restored_segment: Option<SegmentView>,
    pub removed_segment_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayStatsView {
    pub recorded_seconds: i64,
    pub effective_seconds: i64,
    pub blank_seconds: i64,
    pub focus_seconds: i64,
    pub coverage_rate: f64,
    pub switch_count: i64,
    pub segment_count: i64,
    pub average_segment_seconds: i64,
}

impl DayStatsView {
    pub fn new(totals: dotlog_domain::DayTotals, coverage_rate: f64) -> Self {
        Self {
            recorded_seconds: totals.recorded_seconds,
            effective_seconds: totals.effective_seconds,
            blank_seconds: totals.blank_seconds,
            focus_seconds: totals.focus_seconds,
            coverage_rate,
            switch_count: totals.switch_count,
            segment_count: totals.segment_count,
            average_segment_seconds: totals.average_segment_seconds,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GapView {
    pub start_at: i64,
    pub end_at: i64,
    pub seconds: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayView {
    pub date: String,
    pub window_start: i64,
    pub window_end: i64,
    /// 已按日窗口切片；`id` 仍为原始段 id。
    pub segments: Vec<SegmentView>,
    pub gaps: Vec<GapView>,
    pub stats: DayStatsView,
}

/// `segment_update` 的补丁：`None` 表示不修改该字段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SegmentPatch {
    pub title: Option<String>,
    pub note: Option<String>,
    /// 新起点（epoch ms）；active 段只允许改起点。
    pub start_at: Option<i64>,
    /// 新终点（epoch ms）；active 段不可设终点。
    pub end_at: Option<i64>,
    /// 双层 `Option`：缺省 = 不修改，`null` = 清空分类。
    pub category_id: Option<Option<String>>,
    pub is_focus: Option<bool>,
    pub is_ignored: Option<bool>,
}

/// `segment_backfill` 的入参（ipc-contract §3.2）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillInput {
    pub start_at: i64,
    pub end_at: i64,
    pub title: Option<String>,
    pub note: Option<String>,
    pub category_id: Option<String>,
    #[serde(default)]
    pub is_focus: bool,
    #[serde(default)]
    pub is_ignored: bool,
}

/// `history_query` 的结果：倒序段列表 + 符合条件的总数。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    pub items: Vec<SegmentView>,
    pub total: u64,
}

/// `stats_get` 的一个分类占比项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySecondsView {
    pub category_id: Option<String>,
    pub seconds: i64,
}

/// `stats_get` 的单个日 / 周 / 月桶（ipc-contract §3.3）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsBucketView {
    pub date_start: String,
    pub date_end: String,
    pub window_start: i64,
    pub window_end: i64,
    pub recorded_seconds: i64,
    pub effective_seconds: i64,
    pub focus_seconds: i64,
    pub blank_seconds: i64,
    pub coverage_rate: f64,
    pub switch_count: i64,
    pub segment_count: i64,
    pub by_category: Vec<CategorySecondsView>,
}

/// `stats_get` 的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsView {
    pub buckets: Vec<StatsBucketView>,
}

/// 导出内容（由桌面层负责写盘）。
#[derive(Debug, Clone)]
pub struct ExportPayload {
    pub content: String,
    pub count: u64,
}

/// CSV 导入预览的一行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRowView {
    pub index: usize,
    pub start_at: i64,
    pub end_at: i64,
    pub seconds: i64,
    pub title: String,
    pub category_name: Option<String>,
    /// `ok` / `conflict` / `invalid`。
    pub status: &'static str,
    pub reason: Option<String>,
    pub conflict_ids: Vec<String>,
}

/// CSV 导入预览结果（`import_preview`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreviewView {
    pub total: u64,
    pub importable: u64,
    pub conflicts: u64,
    pub invalid: u64,
    pub rows: Vec<ImportRowView>,
}

/// CSV 导入提交结果（`import_commit`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResultView {
    pub inserted: u64,
    pub skipped: u64,
}

/// `recovery_get` 的结果（ipc-contract §2 `RecoveryInfo`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInfoView {
    pub required: bool,
    pub reason: Option<RecoveryReason>,
    pub active_segment: Option<SegmentView>,
    pub detected_at: i64,
    pub options: Vec<RecoveryAction>,
}

impl RecoveryInfoView {
    pub fn none(detected_at: i64) -> Self {
        Self {
            required: false,
            reason: None,
            active_segment: None,
            detected_at,
            options: Vec::new(),
        }
    }
}

/// 诊断包中的单个表计数。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableCountView {
    pub name: String,
    pub count: i64,
}

/// 启动完整性自检的视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityView {
    pub quick_check_ok: bool,
    pub active_count: i64,
    pub overlap_pairs: i64,
    pub longest_open_seconds: i64,
}

impl IntegrityView {
    pub fn from_report(report: crate::ports::IntegrityReport) -> Self {
        Self {
            quick_check_ok: report.quick_check_ok,
            active_count: report.active_count,
            overlap_pairs: report.overlap_pairs,
            longest_open_seconds: report.longest_open_seconds,
        }
    }
}

/// `diagnostics_snapshot` 的内容（由桌面层补充版本与日志后写盘）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsView {
    pub schema_version: i64,
    pub generated_at: i64,
    pub tables: Vec<TableCountView>,
    pub integrity: IntegrityView,
}

/// `begin_session` 的结果：启动核对、完整性自检与当前状态。
///
/// `cutoff_ms` 是恢复时可用的截断点（日边界或最后心跳），只在桌面层内部
/// 缓存，不通过 IPC 暴露。
#[derive(Debug, Clone)]
pub struct StartupReport {
    pub recovery: RecoveryInfoView,
    pub integrity: IntegrityView,
    pub state: TimerStateView,
    pub cutoff_ms: Option<i64>,
}

pub(crate) fn closed_duration_seconds(closed: &Segment) -> i64 {
    match closed.end_at_ms {
        Some(end) => (end - closed.start_at_ms) / 1_000,
        None => 0,
    }
}

pub(crate) fn pending_note(segment: &Segment) -> Option<PendingNoteView> {
    Some(PendingNoteView {
        segment_id: segment.id.clone(),
        start_at: segment.start_at_ms,
        end_at: segment.end_at_ms?,
    })
}
