import { useEffect, useMemo, useState } from "react";
import {
  dayKeyOf,
  dayWindowOf,
  formatDateHeading,
  formatDuration,
  formatTime,
  parseDayKey
} from "../lib/format";
import { ipc, toIpcError, type IpcError, type SegmentView } from "../lib/ipc";
import { ChevronLeftIcon, ChevronRightIcon, HistoryIcon } from "./icons";

const PAGE_SIZE = 50;
const MONTH_SCAN_LIMIT = 200;
const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"] as const;

interface HistoryPageProps {
  /** 计时/编辑变更后自增，触发重新查询。 */
  dayVersion: number;
  onEditSegment: (segment: SegmentView, next: SegmentView | null) => void;
}

interface DayGroup {
  key: string;
  items: SegmentView[];
}

function monthWindow(cursor: Date): { from: number; to: number } {
  const from = dayWindowOf(new Date(cursor.getFullYear(), cursor.getMonth(), 1)).from;
  const to = dayWindowOf(new Date(cursor.getFullYear(), cursor.getMonth() + 1, 1)).from;
  return { from, to };
}

/** 历史页：月历定位 + 倒序列表 + 文本搜索（ipc-contract §3.2 history_query）。 */
export function HistoryPage({ dayVersion, onEditSegment }: HistoryPageProps) {
  const [monthCursor, setMonthCursor] = useState(() => new Date());
  const [selectedDay, setSelectedDay] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<SegmentView[]>([]);
  const [total, setTotal] = useState(0);
  const [monthDays, setMonthDays] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);

  // 搜索输入防抖 300ms。
  useEffect(() => {
    const id = window.setTimeout(() => setQuery(text.trim()), 300);
    return () => window.clearTimeout(id);
  }, [text]);

  const { from: monthFrom, to: monthTo } = useMemo(() => monthWindow(monthCursor), [monthCursor]);

  // 月历圆点：扫一遍当月段，标记有记录的逻辑日。
  useEffect(() => {
    let cancelled = false;
    void ipc
      .historyQuery({ from: monthFrom, to: monthTo, limit: MONTH_SCAN_LIMIT, offset: 0 })
      .then((view) => {
        if (cancelled) return;
        setMonthDays(new Set(view.items.map((segment) => dayKeyOf(segment.startAt))));
      })
      .catch(() => {
        // 圆点失败不阻塞列表。
      });
    return () => {
      cancelled = true;
    };
  }, [monthFrom, monthTo, dayVersion]);

  const listWindow = useMemo(
    () => (selectedDay ? dayWindowOf(parseDayKey(selectedDay)) : { from: monthFrom, to: monthTo }),
    [selectedDay, monthFrom, monthTo]
  );

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void ipc
      .historyQuery({
        from: listWindow.from,
        to: listWindow.to,
        text: query || undefined,
        limit: PAGE_SIZE,
        offset: 0
      })
      .then((view) => {
        if (cancelled) return;
        setItems(view.items);
        setTotal(view.total);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(toIpcError(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [listWindow.from, listWindow.to, query, dayVersion]);

  const loadMore = () => {
    setLoadingMore(true);
    void ipc
      .historyQuery({
        from: listWindow.from,
        to: listWindow.to,
        text: query || undefined,
        limit: PAGE_SIZE,
        offset: items.length
      })
      .then((view) => {
        setItems((prev) => [...prev, ...view.items]);
        setTotal(view.total);
      })
      .catch((cause: unknown) => setError(toIpcError(cause)))
      .finally(() => setLoadingMore(false));
  };

  const shiftMonth = (delta: number) => {
    setMonthCursor((current) => new Date(current.getFullYear(), current.getMonth() + delta, 1));
    setSelectedDay(null);
  };

  // 列表倒序：chronological 下一段是数组里前一项。
  const nextById = new Map<string, SegmentView | null>();
  items.forEach((item, index) => nextById.set(item.id, items[index - 1] ?? null));

  const groups: DayGroup[] = [];
  for (const item of items) {
    const key = dayKeyOf(item.startAt);
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.items.push(item);
    } else {
      groups.push({ key, items: [item] });
    }
  }

  const todayKey = dayKeyOf(Date.now());
  const calendarCells = useMemo(() => {
    const first = new Date(monthCursor.getFullYear(), monthCursor.getMonth(), 1);
    const daysInMonth = new Date(monthCursor.getFullYear(), monthCursor.getMonth() + 1, 0).getDate();
    const leading = (first.getDay() + 6) % 7; // 周一为首列
    const cells: Array<{ key: string; day: number } | null> = Array.from(
      { length: leading },
      () => null
    );
    for (let day = 1; day <= daysInMonth; day += 1) {
      const key = dayKeyOf(
        new Date(monthCursor.getFullYear(), monthCursor.getMonth(), day, 12).getTime()
      );
      cells.push({ key, day });
    }
    return cells;
  }, [monthCursor]);

  const monthTitle = `${monthCursor.getFullYear()} 年 ${monthCursor.getMonth() + 1} 月`;

  return (
    <>
      <header className="page-head">
        <h1>历史</h1>
        <p>
          {selectedDay
            ? `${formatDateHeading(parseDayKey(selectedDay)).full} · ${total} 段`
            : `${monthTitle} · ${total} 段`}
        </p>
      </header>

      <div className="history">
        <aside className="cal" aria-label="月历">
          <div className="cal__head">
            <span className="cal__title">{monthTitle}</span>
            <div className="cal__nav">
              <button
                className="date-nav__btn"
                type="button"
                aria-label="上个月"
                onClick={() => shiftMonth(-1)}
              >
                <ChevronLeftIcon size={14} />
              </button>
              <button
                className="date-nav__btn"
                type="button"
                aria-label="下个月"
                onClick={() => shiftMonth(1)}
              >
                <ChevronRightIcon size={14} />
              </button>
            </div>
          </div>
          <div className="cal__grid">
            {WEEKDAYS.map((day) => (
              <span className="cal__dow" key={day}>
                {day}
              </span>
            ))}
            {calendarCells.map((cell, index) =>
              cell === null ? (
                <span className="cal__day cal__day--pad" key={`pad-${index}`} aria-hidden="true" />
              ) : (
                <button
                  key={cell.key}
                  type="button"
                  className={`cal__day${cell.key === todayKey ? " cal__day--today" : ""}${
                    cell.key === selectedDay ? " cal__day--selected" : ""
                  }${monthDays.has(cell.key) ? " cal__day--dot" : ""}`}
                  aria-pressed={cell.key === selectedDay}
                  aria-label={`${cell.day} 日${monthDays.has(cell.key) ? "，有记录" : ""}`}
                  onClick={() =>
                    setSelectedDay((current) => (current === cell.key ? null : cell.key))
                  }
                >
                  {cell.day}
                </button>
              )
            )}
          </div>
          <input
            className="field__input cal__search"
            type="search"
            value={text}
            placeholder="搜索标题或备注…"
            aria-label="搜索历史记录"
            onChange={(event) => setText(event.target.value)}
          />
        </aside>

        <section className="history__list" aria-label="历史记录列表">
          {error && (
            <div className="modal__error" role="alert">
              {error.message || "读取失败，请重试"}
            </div>
          )}
          {loading ? (
            <div className="timeline-loading" role="status">
              正在读取历史记录…
            </div>
          ) : groups.length === 0 ? (
            <div className="empty-state">
              <span className="empty-state__icon">
                <HistoryIcon size={20} />
              </span>
              <strong>{query ? "没有匹配的记录" : "这段时间还没有记录"}</strong>
              <p>{query ? "换个关键词试试。" : "回到「今天」开始打点，或在空白处补录。"}</p>
            </div>
          ) : (
            <>
              {groups.map((group) => (
                <section className="history-group" key={group.key}>
                  <h3 className="history-group__head">
                    {formatDateHeading(parseDayKey(group.key)).compact}
                    <span className="meta">{group.items.length} 段</span>
                  </h3>
                  <ol className="timeline-list">
                    {group.items.map((segment) => (
                      <li className="timeline-row" key={segment.id}>
                        <time>{formatTime(segment.startAt)}</time>
                        <button
                          className={`segment segment--btn${
                            segment.status === "active" ? " segment--active" : ""
                          }`}
                          type="button"
                          onClick={() =>
                            onEditSegment(segment, nextById.get(segment.id) ?? null)
                          }
                          aria-label={`编辑 ${segment.title || "未命名"}`}
                        >
                          <span className="segment__main">
                            <span
                              className={`segment__title${
                                segment.title ? "" : " segment__title--empty"
                              }`}
                            >
                              {segment.title || "未命名"}
                              {segment.status === "active" && (
                                <span className="badge">进行中</span>
                              )}
                            </span>
                            <span className="segment__meta">
                              {formatTime(segment.startAt)} –{" "}
                              {segment.endAt ? formatTime(segment.endAt) : "现在"}
                              {segment.note && (
                                <span className="segment__note">{segment.note}</span>
                              )}
                            </span>
                          </span>
                          <span className="segment__duration">
                            {formatDuration(segment.durationSeconds)}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ol>
                </section>
              ))}
              {items.length < total && (
                <button
                  className="ghost-btn history__more"
                  type="button"
                  disabled={loadingMore}
                  aria-busy={loadingMore}
                  onClick={loadMore}
                >
                  {loadingMore ? "正在加载…" : `加载更多（还有 ${total - items.length} 段）`}
                </button>
              )}
            </>
          )}
        </section>
      </div>
    </>
  );
}
