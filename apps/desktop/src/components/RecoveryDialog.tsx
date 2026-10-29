import { formatDuration, formatTime } from "../lib/format";
import type { RecoveryAction, RecoveryInfo, RecoveryReason } from "../lib/ipc";

const REASON_COPY: Record<
  RecoveryReason,
  { title: string; description: string; recommended: RecoveryAction }
> = {
  cross_day: {
    title: "有一段记录跨过了今天的起点",
    description: "这段记录开始于昨天，应用关闭时没有结束。请选择如何处理它。",
    recommended: "truncate"
  },
  long_gap: {
    title: "检测到一段长时间的中断",
    description: "休眠或关机期间没有收到心跳，这段记录可能包含空白时间。",
    recommended: "truncate"
  },
  clock_anomaly: {
    title: "系统时间与记录不一致",
    description: "当前时间早于记录起点，或系统时间发生了回拨。请确认这段记录如何结束。",
    recommended: "extend"
  },
  orphan_active: {
    title: "上次没有正常退出",
    description: "应用被强行关闭，留下了一段没有结束的记录。",
    recommended: "extend"
  }
};

function actionLabel(action: RecoveryAction, reason: RecoveryReason): string {
  switch (action) {
    case "truncate":
      return reason === "long_gap" ? "截断到最后心跳" : "截断到今天起点";
    case "extend":
      return "结束到现在";
    case "discard":
      return "丢弃这段记录";
    default:
      return "保留并继续";
  }
}

interface RecoveryDialogProps {
  info: RecoveryInfo;
  busy: boolean;
  onResolve: (action: RecoveryAction) => void;
}

export function RecoveryDialog({ info, busy, onResolve }: RecoveryDialogProps) {
  const reason = info.reason ?? "orphan_active";
  const copy = REASON_COPY[reason];
  const recommended = info.options.includes(copy.recommended)
    ? copy.recommended
    : info.options[0];
  const ordered = [
    ...info.options.filter((action) => action === recommended),
    ...info.options.filter((action) => action !== recommended)
  ];
  const segment = info.activeSegment;

  return (
    <>
      <div className="modal-backdrop" role="presentation" />
      <div
        className="modal modal--recovery"
        role="dialog"
        aria-modal="true"
        aria-labelledby="recovery-title"
        aria-describedby="recovery-desc"
      >
        <header className="modal__head">
          <div>
            <strong id="recovery-title">{copy.title}</strong>
            <span className="modal__range">启动核对 · 需要先处理这段记录</span>
          </div>
        </header>

        <p className="recovery-text" id="recovery-desc">
          {copy.description}
        </p>

        {segment && (
          <dl className="recovery-facts">
            <div>
              <dt>记录内容</dt>
              <dd>{segment.title.trim() || "未命名记录"}</dd>
            </div>
            <div>
              <dt>开始时间</dt>
              <dd>{formatTime(segment.startAt)}</dd>
            </div>
            <div>
              <dt>已持续</dt>
              <dd>
                {formatDuration(
                  Math.max(
                    0,
                    segment.durationSeconds ||
                      Math.floor((Date.now() - segment.startAt) / 1000)
                  )
                )}
              </dd>
            </div>
          </dl>
        )}

        <footer className="modal__foot">
          <p className="modal__hint">选择后立即写入，不可撤销</p>
          <div className="modal__actions">
            {ordered.map((action) => (
              <button
                key={action}
                type="button"
                className={
                  action === recommended
                    ? "primary-btn primary-btn--sm"
                    : action === "discard"
                      ? "danger-btn danger-btn--plain"
                      : "ghost-btn"
                }
                disabled={busy}
                onClick={() => onResolve(action)}
              >
                {actionLabel(action, reason)}
              </button>
            ))}
          </div>
        </footer>
      </div>
    </>
  );
}
