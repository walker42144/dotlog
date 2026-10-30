import { formatClock, formatTime } from "../lib/format";
import type { TimerState } from "../lib/ipc";

interface StatusPanelProps {
  state: TimerState | null;
  elapsed: number;
  pending: boolean;
  onStart: () => void;
  onPunch: () => void;
  onPause: () => void;
  onFinish: () => void;
}

export function StatusPanel({
  state,
  elapsed,
  pending,
  onStart,
  onPunch,
  onPause,
  onFinish
}: StatusPanelProps) {
  const active = state?.active ?? null;
  const recording = state?.phase === "recording" && active !== null;
  const paused = state?.paused === true;
  const tone = recording ? "recording" : paused ? "paused" : "idle";
  const label = recording ? "记录中" : paused ? "暂停中" : "未开始";
  const modeLabel = state?.mode === "chain" ? "连续打点" : "打点即停";

  const hint = recording
    ? `本次开始于 ${formatTime(state?.sessionStartedAt ?? active.startAt)}`
    : paused
      ? "计时已冻结，继续后累计；暂停时间不计入时长"
      : state
        ? "点击开始，记录此刻"
        : "正在读取本地数据…";

  return (
    <section className="status-card" data-phase={tone} aria-label="当前记录状态">
      <div>
        <div className="status-card__meta">
          <span className={`status-dot status-dot--${tone}`} />
          <span className="status-card__label" key={tone}>{label}</span>
          <span className="status-card__hint">{hint}</span>
        </div>

        <div className="status-card__elapsed">
          {recording || paused ? (
            <>
              <span className="elapsed">{formatClock(elapsed)}</span>
              <span className="elapsed__unit">{paused ? "已暂停 · 累计" : "实际记录"}</span>
            </>
          ) : (
            <span className="elapsed elapsed--idle">{paused ? "已暂停" : "尚未开始"}</span>
          )}
        </div>

        <p className="status-card__sub">
          {recording
            ? `本次从 ${formatTime(state?.sessionStartedAt ?? active.startAt)} 开始 · ${modeLabel}`
            : `记录模式：${modeLabel}`}
        </p>
      </div>

      <div className="status-card__actions">
        <button
          className="primary-btn"
          type="button"
          disabled={pending || !state}
          aria-busy={pending}
          onClick={recording ? onPunch : onStart}
        >
          {recording ? "打点" : paused ? "继续" : "开始打点"}
        </button>

        {recording || paused ? (
          <div className="secondary-row">
            {recording && <button className="ghost-btn" type="button" disabled={pending} onClick={onPause}>
              暂停
            </button>}
            <button className="ghost-btn" type="button" disabled={pending} onClick={onFinish}>
              结束记录
            </button>
          </div>
        ) : (
          <div className="kbd-hint">
            全局快捷键可在设置中查看
          </div>
        )}
      </div>
    </section>
  );
}
