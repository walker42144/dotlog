import { formatClock, formatDuration, formatTime } from "../lib/format";
import type { DayView, TimerState } from "../lib/ipc";
import { LockIcon } from "./icons";

interface StatsAsideProps {
  state: TimerState | null;
  elapsed: number;
  dateLabel: string;
  day: DayView | null;
  loading: boolean;
}

export function StatsAside({ state, elapsed, dateLabel, day, loading }: StatsAsideProps) {
  const active = state?.active ?? null;
  const modeLabel = state?.mode === "chain" ? "连续打点" : "打点即停";
  const stats = day?.stats;
  const coverage = Math.round((stats?.coverageRate ?? 0) * 100);
  const activeSegment = day?.segments.find((segment) => segment.status === "active") ?? null;
  // 日视图拉取后计时仍在增长：用本地秒表替换进行中段的时长。
  const liveRecorded =
    (stats?.recordedSeconds ?? 0) -
    (activeSegment?.durationSeconds ?? 0) +
    (activeSegment ? elapsed : 0);

  return (
    <aside className="stats" aria-label="今日概览">
      <div className="stats__head">
        <h2>今日概览</h2>
        <span>{dateLabel}</span>
      </div>

      <section className="panel">
        <h3>当前记录</h3>
        <div className="metric-hero">
          <span className="metric-hero__value">{active ? formatClock(elapsed) : "00:00:00"}</span>
          <span className="metric-hero__unit">{active ? "已记录" : "未开始"}</span>
        </div>
        <dl className="kv-list">
          <div className="kv-row">
            <dt>开始时间</dt>
            <dd>{active ? formatTime(active.startAt) : "—"}</dd>
          </div>
          <div className="kv-row">
            <dt>记录模式</dt>
            <dd className="kv-row__plain">{modeLabel}</dd>
          </div>
        </dl>
      </section>

      <section className="panel">
        <h3>今日统计</h3>
        <div className="metric-hero">
          <span className="metric-hero__value">{formatClock(liveRecorded)}</span>
          <span className="metric-hero__unit">
            {loading && !day ? "读取中" : `共 ${stats?.segmentCount ?? 0} 段`}
          </span>
        </div>
        <dl className="kv-list">
          <div className="kv-row">
            <dt>有效记录</dt>
            <dd>{formatDuration(stats?.effectiveSeconds ?? 0)}</dd>
          </div>
          <div className="kv-row">
            <dt>空白</dt>
            <dd>{formatDuration(stats?.blankSeconds ?? 0)}</dd>
          </div>
        </dl>

        <div className="coverage">
          <div className="coverage__head">
            <span>记录完整率</span>
            <span className="coverage__value">{coverage}%</span>
          </div>
          <div
            className="coverage__track"
            role="progressbar"
            aria-label="记录完整率"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={coverage}
          >
            <span className="coverage__fill" style={{ width: `${coverage}%` }} />
          </div>
        </div>
      </section>

      <section className="panel">
        <h3>分类占比</h3>
        <div className="ring-wrap">
          <svg className="ring-empty" viewBox="0 0 96 96" aria-hidden="true">
            <circle cx="48" cy="48" r="38" fill="none" stroke="var(--border)" strokeWidth="8" />
            <circle
              cx="48"
              cy="48"
              r="38"
              fill="none"
              stroke="var(--border-strong)"
              strokeWidth="8"
              strokeDasharray="1 7"
              strokeLinecap="round"
            />
            <text
              x="48"
              y="52"
              textAnchor="middle"
              fill="var(--text-faint)"
              fontSize="11"
              fontFamily="var(--font-sans)"
            >
              暂无
            </text>
          </svg>
          <span className="ring-caption">分类在时间线编辑版本加入</span>
        </div>
      </section>

      <p className="stats__foot">
        <LockIcon />
        全部数据只保存在本机
      </p>
    </aside>
  );
}
