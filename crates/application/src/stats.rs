//! 统计用例：日 / 周 / 月桶（`docs/ipc-contract.md` §3.3，口径见 domain-model §10）。
//!
//! 计算全部委托 `dotlog-domain::stats`，本模块只做窗口编排与视图映射。

use dotlog_domain::{bucket_windows, local_date, stats_bucket, Granularity};

use crate::dto::{CategorySecondsView, StatsBucketView, StatsView};
use crate::error::{AppError, AppResult};
use crate::ports::{Clock, TimerStore};
use crate::service::TimerService;

/// 单次查询最多的桶数，防止异常区间拖垮 UI。
const MAX_BUCKETS: usize = 400;

impl<S: TimerStore, C: Clock> TimerService<S, C> {
    /// `stats_get`：按粒度切桶，当前桶只统计到 now。
    pub fn stats(&self, from_ms: i64, to_ms: i64, granularity: Granularity) -> AppResult<StatsView> {
        if from_ms >= to_ms {
            return Err(AppError::store("E_INVALID_RANGE", "统计区间起止非法"));
        }
        let now = self.clock.now();
        let policy = self.day_policy()?;
        let windows = bucket_windows(
            from_ms,
            to_ms,
            granularity,
            now.tz_offset_min,
            policy.day_start_minutes,
        );
        if windows.len() > MAX_BUCKETS {
            return Err(AppError::store("E_INVALID_RANGE", "统计区间过长"));
        }

        let segments = self.store.day_segments(from_ms, to_ms)?;
        let mut buckets = Vec::with_capacity(windows.len());
        for (window_start, window_end) in windows {
            let capped_end = window_end.min(now.utc_ms);
            if capped_end <= window_start {
                continue;
            }
            let stats = stats_bucket(&segments, window_start, capped_end, now.utc_ms, policy);
            buckets.push(StatsBucketView {
                date_start: local_date(
                    window_start,
                    now.tz_offset_min,
                    policy.day_start_minutes,
                ),
                date_end: local_date(
                    capped_end - 1,
                    now.tz_offset_min,
                    policy.day_start_minutes,
                ),
                window_start,
                window_end: capped_end,
                recorded_seconds: stats.recorded_seconds,
                effective_seconds: stats.effective_seconds,
                focus_seconds: stats.focus_seconds,
                blank_seconds: stats.blank_seconds,
                coverage_rate: stats.coverage_rate,
                switch_count: stats.switch_count,
                segment_count: stats.segment_count,
                by_category: stats
                    .by_category
                    .into_iter()
                    .map(|item| CategorySecondsView {
                        category_id: item.category_id,
                        seconds: item.seconds,
                    })
                    .collect(),
            });
        }
        Ok(StatsView { buckets })
    }
}
