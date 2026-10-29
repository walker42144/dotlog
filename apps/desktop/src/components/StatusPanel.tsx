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
    ? `本次开始于 ${formatTime(active.startAt)}`
    : paused
      ? "上一段已关闭，继续会开启新的一段"
      : state
        ? "点击开始，记录此刻"
        : "正在读取本地数据…";

  return (
    <section className="status-card" aria-label="当前记录状态">
      <div>
        <div className="status-card__meta">
          <span className={`status-dot status-dot--${tone}`} />
          <span className="status-card__label">{label}</span>
          <span className="status-card__hint">{hint}</span>
        </div>

        <div className="status-card__elapsed">
          {recording ? (
            <>
              <span className="elapsed">{formatClock(elapsed)}</span>
              <span className="elapsed__unit">已记录</span>
            </>
          ) : (
            <span className="elapsed elapsed--idle">{paused ? "已暂停" : "尚未开始"}</span>
          )}
        </div>

        <p className="status-card__sub">
          {recording
            ? `本次从 ${formatTime(active.startAt)} 开始 · ${modeLabel}`
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

        {recording ? (
          <div className="secondary-row">
            <button className="ghost-btn" type="button" disabled={pending} onClick={onPause}>
              暂停
            </button>
            <button className="ghost-btn" type="button" disabled={pending} onClick={onFinish}>
              结束记录
            </button>
          </div>
        ) : (
          <div className="kbd-hint">
            快捷键 <kbd>Ctrl</kbd> + <kbd>Alt</kbd> + <kbd>Space</kbd>
          </div>
        )}
      </div>
    </section>
  );
}
