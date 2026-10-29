import { formatClock, formatDuration, formatTime } from "../lib/format";
import type { DayView, TimerState } from "../lib/ipc";
import { LockIcon } from "./icons";
interface StatsAsideProps { state: TimerState | null; elapsed: number; dateLabel: string; day: DayView | null; loading: boolean }
export function StatsAside({ state, elapsed, dateLabel, day, loading }: StatsAsideProps) {
  const active = state?.active ?? null;
  const now = Date.now();
  const duration = (s: NonNullable<DayView>["segments"][number]) => s.status === "active" && day ? Math.max(0, Math.floor((Math.min(now, day.windowEnd) - Math.max(s.startAt, day.windowStart)) / 1000)) : s.durationSeconds;
  const rows = day?.segments ?? [];
  const recorded = rows.reduce((n, s) => n + duration(s), 0);
  const effective = rows.filter((s) => !s.isIgnored && (s.title || s.category)).reduce((n, s) => n + duration(s), 0);
  const windowSeconds = day ? Math.max(0, Math.floor((Math.min(now, day.windowEnd) - day.windowStart) / 1000)) : 0;
  const blank = Math.max(0, windowSeconds - recorded);
  const coverage = windowSeconds > 0 ? Math.min(100, Math.round(effective / windowSeconds * 100)) : 0;
  const categories = new Map<string, { name: string; color: string; seconds: number }>();
  for (const segment of rows.filter((s) => !s.isIgnored)) {
    const key = segment.category?.id ?? "uncategorized";
    const previous = categories.get(key);
    categories.set(key, { name: segment.category?.name ?? "未分类", color: segment.category?.color ?? "var(--text-muted)", seconds: (previous?.seconds ?? 0) + duration(segment) });
  }
  const distribution = [...categories.values()].sort((a, b) => b.seconds - a.seconds);
  const categoryTotal = distribution.reduce((n, row) => n + row.seconds, 0);
  return <aside className="stats" aria-label="今日概览">
    <div className="stats__head"><h2>今日概览</h2><span>{dateLabel}</span></div>
    <section className="panel"><h3>当前记录</h3><div className="metric-hero"><span className="metric-hero__value">{formatClock(active || state?.paused ? elapsed : 0)}</span><span className="metric-hero__unit">{active ? "实际记录" : state?.paused ? "暂停 · 累计" : "未开始"}</span></div><dl className="kv-list"><div className="kv-row"><dt>开始时间</dt><dd>{state?.sessionStartedAt ? formatTime(state.sessionStartedAt) : active ? formatTime(active.startAt) : "—"}</dd></div><div className="kv-row"><dt>记录模式</dt><dd>{state?.mode === "chain" ? "连续打点" : "打点即停"}</dd></div></dl></section>
    <section className="panel"><h3>今日统计</h3><div className="metric-hero"><span className="metric-hero__value">{formatClock(recorded)}</span><span className="metric-hero__unit">{loading && !day ? "读取中" : `共 ${rows.length} 段`}</span></div><dl className="kv-list"><div className="kv-row"><dt>有效记录</dt><dd>{formatDuration(effective)}</dd></div><div className="kv-row"><dt>空白</dt><dd>{formatDuration(blank)}</dd></div></dl><div className="coverage"><div className="coverage__head"><span>记录完整率</span><span>{coverage}%</span></div><div className="coverage__track" role="progressbar" aria-label="记录完整率" aria-valuemin={0} aria-valuemax={100} aria-valuenow={coverage}><span className="coverage__fill" style={{ width: `${coverage}%` }} /></div></div></section>
    <section className="panel"><h3>分类占比</h3>{distribution.length ? distribution.map((row) => <div key={row.name} className="category-meter"><div className="coverage__head"><span>{row.name}</span><span>{formatDuration(row.seconds)}</span></div><div className="coverage__track"><span className="coverage__fill" style={{ width: `${categoryTotal ? row.seconds / categoryTotal * 100 : 0}%`, background: row.color }} /></div></div>) : <p className="panel__empty">暂无分类记录</p>}</section>
    <p className="stats__foot"><LockIcon />记录保存在本机 · 联网集成按配置运行</p>
  </aside>;
}
