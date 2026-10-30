import { formatDuration, formatTime } from "../lib/format";
import type { DayView, SegmentView } from "../lib/ipc";
import { TimelineIcon } from "./icons";

interface TimelineProps {
  day: DayView | null;
  loading: boolean;
  elapsed: number;
  onStart: () => void;
  onEditSegment: (segment: SegmentView, next: SegmentView | null) => void;
  onBackfillGap: (startAt: number, endAt: number) => void;
}

type Row =
  | { kind: "segment"; key: string; startAt: number; segment: SegmentView }
  | { kind: "gap"; key: string; startAt: number; endAt: number; seconds: number };

/** 小于 2 分钟的空白在视觉上并入相邻段（统计仍按真实空白计算）。 */
const VISIBLE_GAP_SECONDS = 120;

function buildRows(day: DayView | null): Row[] {
  if (!day) return [];
  const rows: Row[] = day.segments.map((segment) => ({
    kind: "segment",
    key: `seg-${segment.id}`,
    startAt: segment.startAt,
    segment
  }));
  for (const gap of day.gaps) {
    if (gap.seconds < VISIBLE_GAP_SECONDS) continue;
    rows.push({
      kind: "gap",
      key: `gap-${gap.startAt}`,
      startAt: gap.startAt,
      endAt: gap.endAt,
      seconds: gap.seconds
    });
  }
  return rows.sort((left, right) => left.startAt - right.startAt);
}

/** 每段的下一段（按起点排序），供「并入下一段」使用。 */
function buildNextMap(day: DayView | null): Map<string, SegmentView | null> {
  const map = new Map<string, SegmentView | null>();
  const segments = [...(day?.segments ?? [])].sort((a, b) => a.startAt - b.startAt);
  for (let index = 0; index < segments.length; index += 1) {
    map.set(segments[index].id, segments[index + 1] ?? null);
  }
  return map;
}

function SegmentRow({
  segment,
  elapsed,
  onEdit
}: {
  segment: SegmentView;
  elapsed: number;
  onEdit: () => void;
}) {
  const active = segment.status === "active";
  const duration = active ? elapsed : segment.durationSeconds;

  return (
    <button
      className={`segment segment--btn${active ? " segment--active" : ""}`}
      type="button"
      onClick={onEdit}
      aria-label={`编辑 ${segment.title || "未命名"}`}
    >
      <span className="segment__main">
        <span className={`segment__title${segment.title ? "" : " segment__title--empty"}`}>
          {segment.title || "未命名"}
          {active && <span className="badge">进行中</span>}
        </span>
        <span className="segment__meta">
          {formatTime(segment.startAt)} – {segment.endAt ? formatTime(segment.endAt) : "现在"}
          {segment.note && <span className="segment__note">{segment.note}</span>}
        </span>
      </span>
      <span className="segment__duration">{formatDuration(duration)}</span>
    </button>
  );
}

export function Timeline({
  day,
  loading,
  elapsed,
  onStart,
  onEditSegment,
  onBackfillGap
}: TimelineProps) {
  const rows = buildRows(day);
  const nextById = buildNextMap(day);
  const segmentCount = day?.segments.length ?? 0;
  const activeSegment = day?.segments.find((segment) => segment.status === "active") ?? null;
  const activeElapsed = activeSegment && day ? Math.min(elapsed, Math.max(0, Math.floor((Math.min(Date.now(), day.windowEnd) - Math.max(activeSegment.startAt, day.windowStart)) / 1000))) : 0;
  // The headline uses this slice, while the main timer accumulates the whole session.
  const liveRecorded =
    (day?.stats.recordedSeconds ?? 0) -
    (activeSegment?.durationSeconds ?? 0) +
    activeElapsed;

  return (
    <section aria-label="今日时间线">
      <div className="section-head">
        <h2>时间线</h2>
        <span className="meta">
          {loading && !day
            ? "正在读取…"
            : segmentCount > 0
              ? `${segmentCount} 段 · 记录 ${formatDuration(liveRecorded)}`
              : "今天还没有记录"}
        </span>
      </div>

      <div className="timeline">
        <div className="timeline__body">
          {rows.length > 0 ? (
            <ol className="timeline-list">
              {rows.map((row) =>
                row.kind === "segment" ? (
                  <li className="timeline-row" key={row.key}>
                    <time>{formatTime(row.segment.startAt)}</time>
                    <SegmentRow
                      segment={row.segment}
                      elapsed={activeElapsed}
                      onEdit={() => onEditSegment(row.segment, nextById.get(row.segment.id) ?? null)}
                    />
                  </li>
                ) : (
                  <li className="timeline-row" key={row.key}>
                    <time>{formatTime(row.startAt)}</time>
                    <button
                      className="gap-block gap-block--btn"
                      type="button"
                      onClick={() => onBackfillGap(row.startAt, row.endAt)}
                      aria-label={`补录 ${formatTime(row.startAt)} 到 ${formatTime(row.endAt)} 的空白`}
                    >
                      <span className="gap-block__label">
                        空白 {formatDuration(row.seconds)}
                        <span className="gap-block__range">
                          {formatTime(row.startAt)} – {formatTime(row.endAt)}
                        </span>
                      </span>
                      <span className="gap-block__hint">点击补录</span>
                    </button>
                  </li>
                )
              )}
            </ol>
          ) : loading && !day ? (
            <div className="timeline-loading" role="status">
              正在读取今天的记录…
            </div>
          ) : (
            <div className="empty-state">
              <span className="empty-state__icon">
                <TimelineIcon size={20} />
              </span>
              <strong>时间线还是空的</strong>
              <p>开始打点后，每段记录会按时间顺序出现在这里；空白时段也可以补录。</p>
              <button className="link-btn" type="button" onClick={onStart}>
                现在开始打点
              </button>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
