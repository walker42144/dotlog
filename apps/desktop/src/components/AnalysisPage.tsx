import { useCallback, useEffect, useMemo, useState } from "react";
import {
  DAY_START_MINUTES,
  dayWindowOf,
  formatDuration,
  formatTime
} from "../lib/format";
import {
  friendlyMessage,
  ipc,
  toIpcError,
  type CategoryView,
  type Granularity,
  type SegmentView,
  type StatsBucket
} from "../lib/ipc";
import { AnalysisIcon } from "./icons";

const DAY_MS = 86_400_000;

const GRANULARITIES: Array<{ key: Granularity; label: string }> = [
  { key: "day", label: "日" },
  { key: "week", label: "周" },
  { key: "month", label: "月" }
];

const RANGE_COUNTS: Record<Granularity, number[]> = {
  day: [7, 14, 30],
  week: [4, 8, 12],
  month: [3, 6, 12]
};

const DEFAULT_COUNT: Record<Granularity, number> = { day: 7, week: 8, month: 6 };

/** `[from, to)`：以当前逻辑日为终点，按粒度向前回溯 `count` 个桶。 */
function rangeOf(granularity: Granularity, count: number): { from: number; to: number } {
  const startToday = dayWindowOf(new Date()).from;
  if (granularity === "day") {
    return { from: startToday - (count - 1) * DAY_MS, to: startToday + DAY_MS };
  }
  if (granularity === "week") {
    const weekday = (new Date(startToday).getDay() + 6) % 7;
    const monday = startToday - weekday * DAY_MS;
    return { from: monday - (count - 1) * 7 * DAY_MS, to: monday + 7 * DAY_MS };
  }
  const now = new Date();
  const monthStart = (offset: number) =>
    new Date(now.getFullYear(), now.getMonth() + offset, 1).getTime() +
    DAY_START_MINUTES * 60_000;
  return { from: monthStart(-(count - 1)), to: monthStart(1) };
}

function bucketLabel(bucket: StatsBucket, granularity: Granularity): string {
  const start = bucket.dateStart.slice(5).replace("-", "/");
  if (granularity === "month") return bucket.dateStart.slice(0, 7).replace("-", "/");
  if (granularity === "week") return `${start} 周`;
  return start;
}

function rangeLabel(from: string, to: string): string {
  return from === to ? from : `${from} → ${to}`;
}

function longestFocusOf(items: SegmentView[]): SegmentView | null {
  let best: SegmentView | null = null;
  for (const item of items) {
    if (!item.isFocus) continue;
    if (!best || item.durationSeconds > best.durationSeconds) best = item;
  }
  return best;
}

interface AnalysisPageProps {
  dayVersion: number;
}

export function AnalysisPage({ dayVersion }: AnalysisPageProps) {
  const [granularity, setGranularity] = useState<Granularity>("day");
  const [count, setCount] = useState(DEFAULT_COUNT.day);
  const [buckets, setBuckets] = useState<StatsBucket[]>([]);
  const [categories, setCategories] = useState<CategoryView[]>([]);
  const [longestFocus, setLongestFocus] = useState<SegmentView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [drill, setDrill] = useState<SegmentView[] | null>(null);

  const range = useMemo(() => rangeOf(granularity, count), [granularity, count, dayVersion]);

  useEffect(() => {
    ipc
      .categories()
      .then(setCategories)
      .catch(() => setCategories([]));
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    Promise.all([
      ipc.statsGet(range.from, range.to, granularity),
      ipc.historyQuery({ from: range.from, to: range.to, limit: 500 })
    ])
      .then(([stats, history]) => {
        if (cancelled) return;
        setBuckets(stats.buckets);
        setLongestFocus(longestFocusOf(history.items));
        setError(null);
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setError(friendlyMessage(toIpcError(reason)));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [granularity, count, range.from, range.to, dayVersion]);

  useEffect(() => {
    setSelected(null);
    setDrill(null);
  }, [granularity, count]);

  const categoryMap = useMemo(
    () => new Map(categories.map((category) => [category.id, category])),
    [categories]
  );

  const colorOf = useCallback(
    (categoryId: string | null): string => {
      if (!categoryId) return "var(--cat-8)";
      return categoryMap.get(categoryId)?.color ?? "var(--cat-8)";
    },
    [categoryMap]
  );

  const nameOf = useCallback(
    (categoryId: string | null): string => {
      if (!categoryId) return "未分类";
      return categoryMap.get(categoryId)?.name ?? "已删除分类";
    },
    [categoryMap]
  );

  const totals = useMemo(() => {
    let recorded = 0;
    let effective = 0;
    let focus = 0;
    let windowSeconds = 0;
    let segments = 0;
    for (const bucket of buckets) {
      recorded += bucket.recordedSeconds;
      effective += bucket.effectiveSeconds;
      focus += bucket.focusSeconds;
      segments += bucket.segmentCount;
      windowSeconds += Math.floor((bucket.windowEnd - bucket.windowStart) / 1_000);
    }
    return {
      recorded,
      effective,
      focus,
      segments,
      coverage: windowSeconds > 0 ? Math.min(1, effective / windowSeconds) : 0
    };
  }, [buckets]);

  const share = useMemo(() => {
    const sums = new Map<string | null, number>();
    for (const bucket of buckets) {
      for (const item of bucket.byCategory) {
        sums.set(item.categoryId, (sums.get(item.categoryId) ?? 0) + item.seconds);
      }
    }
    return [...sums.entries()]
      .map(([id, seconds]) => ({
        id,
        seconds,
        name: nameOf(id),
        color: colorOf(id)
      }))
      .filter((item) => item.seconds > 0)
      .sort((left, right) => right.seconds - left.seconds);
  }, [buckets, colorOf, nameOf]);

  const shareTotal = share.reduce((sum, item) => sum + item.seconds, 0);

  const maxBucket = useMemo(
    () =>
      buckets.reduce(
        (max, bucket) => Math.max(max, bucket.byCategory.reduce((sum, item) => sum + item.seconds, 0)),
        0
      ),
    [buckets]
  );

  const openBucket = useCallback((index: number, bucket: StatsBucket) => {
    setSelected(index);
    setDrill(null);
    ipc
      .historyQuery({ from: bucket.windowStart, to: bucket.windowEnd, limit: 200 })
      .then((view) => setDrill(view.items))
      .catch(() => setDrill([]));
  }, []);

  const selectedBucket = selected !== null ? buckets[selected] : null;

  return (
    <div className="analysis">
      <header className="page-head">
        <h1>分析</h1>
        <p>统计口径与原始记录逐项对齐 · 忽略段不计入</p>
      </header>

      <div className="analysis__toolbar">
        <div className="seg-tabs" role="tablist" aria-label="统计粒度">
          {GRANULARITIES.map((item) => (
            <button
              key={item.key}
              type="button"
              role="tab"
              className="seg-tab"
              aria-selected={granularity === item.key}
              onClick={() => {
                setGranularity(item.key);
                setCount(DEFAULT_COUNT[item.key]);
              }}
            >
              {item.label}
            </button>
          ))}
        </div>

        <label className="analysis__range">
          <span>区间</span>
          <select
            className="field__input"
            value={count}
            onChange={(event) => setCount(Number(event.target.value))}
          >
            {RANGE_COUNTS[granularity].map((option) => (
              <option key={option} value={option}>
                最近 {option} 个{GRANULARITIES.find((item) => item.key === granularity)?.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      {error && <p className="modal__error">{error}</p>}

      <div className="kpi-row">
        <div className="kpi">
          <span className="kpi__label">记录</span>
          <span className="kpi__value">{formatDuration(totals.recorded)}</span>
        </div>
        <div className="kpi">
          <span className="kpi__label">有效</span>
          <span className="kpi__value">{formatDuration(totals.effective)}</span>
        </div>
        <div className="kpi">
          <span className="kpi__label">专注</span>
          <span className="kpi__value">{formatDuration(totals.focus)}</span>
        </div>
        <div className="kpi">
          <span className="kpi__label">完整率</span>
          <span className="kpi__value">{Math.round(totals.coverage * 100)}%</span>
        </div>
      </div>

      <section className="panel analysis__chart-panel">
        <div className="section-head">
          <h2>分类堆叠</h2>
          <span className="meta">{loading ? "读取中" : `${buckets.length} 个区间 · ${totals.segments} 段`}</span>
        </div>

        {maxBucket > 0 ? (
          <>
            <svg
              className="chart-svg"
              viewBox="0 0 1000 200"
              preserveAspectRatio="none"
              role="img"
              aria-label="按分类的堆叠柱状图"
            >
              {buckets.map((bucket, index) => {
                const step = 1000 / buckets.length;
                const width = Math.min(step * 0.62, 44);
                const x = index * step + (step - width) / 2;
                let cursor = 200;
                return (
                  <g
                    key={bucket.windowStart}
                    className={`chart-bar${selected === index ? " chart-bar--active" : ""}`}
                    onClick={() => openBucket(index, bucket)}
                  >
                    <title>
                      {rangeLabel(bucket.dateStart, bucket.dateEnd)} ·{" "}
                      {formatDuration(bucket.recordedSeconds)}
                    </title>
                    <rect
                      x={x - 2}
                      y={0}
                      width={width + 4}
                      height={200}
                      fill="transparent"
                    />
                    {bucket.byCategory.map((item) => {
                      const height = (item.seconds / maxBucket) * 192;
                      cursor -= height;
                      return (
                        <rect
                          key={`${bucket.windowStart}-${item.categoryId ?? "none"}`}
                          x={x}
                          y={cursor}
                          width={width}
                          height={Math.max(1, height)}
                          fill={colorOf(item.categoryId)}
                          rx={2}
                        />
                      );
                    })}
                  </g>
                );
              })}
            </svg>
            <div
              className="chart-axis"
              style={{ gridTemplateColumns: `repeat(${buckets.length}, minmax(0, 1fr))` }}
            >
              {buckets.map((bucket, index) => {
                const stride = Math.ceil(buckets.length / 12);
                const show = index % stride === 0 || index === buckets.length - 1;
                return (
                  <span key={bucket.windowStart} className="chart-axis__label">
                    {show ? bucketLabel(bucket, granularity) : ""}
                  </span>
                );
              })}
            </div>
          </>
        ) : (
          <div className="empty-state">
            <span className="empty-state__icon">
              <AnalysisIcon />
            </span>
            <strong>这个区间还没有可统计的记录</strong>
            <p>回到「今天」记录几段，或先到历史页补记。</p>
          </div>
        )}
      </section>

      <div className="analysis__grid">
        <section className="panel">
          <h3>分类占比</h3>
          {shareTotal > 0 ? (
            <div className="share">
              <svg className="share__ring" viewBox="0 0 140 140" role="img" aria-label="分类占比环形图">
                <circle
                  cx="70"
                  cy="70"
                  r="52"
                  fill="none"
                  stroke="var(--border)"
                  strokeWidth="12"
                />
                {(() => {
                  const circumference = 2 * Math.PI * 52;
                  let offset = 0;
                  return share.map((item) => {
                    const fraction = item.seconds / shareTotal;
                    const dash = `${fraction * circumference} ${circumference}`;
                    const element = (
                      <circle
                        key={item.id ?? "none"}
                        cx="70"
                        cy="70"
                        r="52"
                        fill="none"
                        stroke={item.color}
                        strokeWidth="12"
                        strokeDasharray={dash}
                        strokeDashoffset={-offset * circumference}
                        transform="rotate(-90 70 70)"
                      />
                    );
                    offset += fraction;
                    return element;
                  });
                })()}
                <text
                  x="70"
                  y="66"
                  textAnchor="middle"
                  fill="var(--text)"
                  fontSize="15"
                  fontFamily="var(--font-sans)"
                >
                  {formatDuration(shareTotal)}
                </text>
                <text
                  x="70"
                  y="84"
                  textAnchor="middle"
                  fill="var(--text-muted)"
                  fontSize="10"
                  fontFamily="var(--font-sans)"
                >
                  已分类
                </text>
              </svg>
              <ul className="legend">
                {share.map((item) => (
                  <li key={item.id ?? "none"} className="legend__item">
                    <span className="legend__dot" style={{ background: item.color }} />
                    <span className="legend__name">{item.name}</span>
                    <span className="legend__value">
                      {Math.round((item.seconds / shareTotal) * 100)}%
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <p className="panel__empty">还没有带分类的记录</p>
          )}
        </section>

        <section className="panel">
          <h3>辅助指标</h3>
          <dl className="kv-list">
            <div className="kv-row">
              <dt>空白时长</dt>
              <dd>
                {formatDuration(
                  buckets.reduce((sum, bucket) => sum + bucket.blankSeconds, 0)
                )}
              </dd>
            </div>
            <div className="kv-row">
              <dt>最长专注段</dt>
              <dd>
                {longestFocus
                  ? `${formatDuration(longestFocus.durationSeconds)} · ${longestFocus.title || "未命名"}`
                  : "—"}
              </dd>
            </div>
            <div className="kv-row">
              <dt>平均完整率</dt>
              <dd>{Math.round(totals.coverage * 100)}%</dd>
            </div>
          </dl>

          <div className="spark">
            <span className="spark__label">完整率趋势</span>
            <svg className="spark__svg" viewBox="0 0 200 56" preserveAspectRatio="none" aria-hidden="true">
              <polyline
                points={buckets
                  .map((bucket, index) => {
                    const x = buckets.length > 1 ? (index / (buckets.length - 1)) * 200 : 100;
                    const y = 52 - bucket.coverageRate * 48;
                    return `${x},${y}`;
                  })
                  .join(" ")}
                fill="none"
                stroke="var(--brand)"
                strokeWidth="1.5"
              />
            </svg>
          </div>
        </section>
      </div>

      {selectedBucket && (
        <div className="drawer-backdrop" onClick={() => setSelected(null)}>
          <aside
            className="drawer"
            aria-label="区间明细"
            onClick={(event) => event.stopPropagation()}
          >
            <header className="drawer__head">
              <div>
                <strong>{rangeLabel(selectedBucket.dateStart, selectedBucket.dateEnd)}</strong>
                <span className="drawer__meta">
                  {formatDuration(selectedBucket.recordedSeconds)} · {selectedBucket.segmentCount} 段
                </span>
              </div>
              <button
                type="button"
                className="drawer__close"
                aria-label="关闭"
                onClick={() => setSelected(null)}
              >
                ×
              </button>
            </header>

            {drill === null ? (
              <p className="panel__empty">读取中…</p>
            ) : drill.length === 0 ? (
              <p className="panel__empty">这个区间没有记录</p>
            ) : (
              <ul className="drawer__list">
                {drill.map((segment) => (
                  <li key={segment.id} className="drawer__row">
                    <time>{formatTime(segment.startAt)}</time>
                    <span className="drawer__title">{segment.title || "未命名"}</span>
                    <span className="drawer__duration">
                      {formatDuration(segment.durationSeconds)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </aside>
        </div>
      )}
    </div>
  );
}
