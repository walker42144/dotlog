const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"] as const;

/** 秒 → “42 分 / 1 小时 24 分”，用于辅助信息。 */
export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  if (total < 60) return `${total} 秒`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} 小时` : `${hours} 小时 ${rest} 分`;
}

/** 秒 → HH:MM:SS，等宽显示防跳动。 */
export function formatClock(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return [h, m, s].map((part) => String(part).padStart(2, "0")).join(":");
}

const timeFormatter = new Intl.DateTimeFormat("zh-CN", {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false
});

export function formatTime(epochMs: number): string {
  return timeFormatter.format(epochMs);
}

/** datetime-local 输入框值 → epoch 毫秒；非法输入返回 null。 */
export function fromLocalInput(value: string): number | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isNaN(ms) ? null : ms;
}

/** epoch 毫秒 → datetime-local 输入框值（本地时区，分钟精度）。 */
export function toLocalInput(epochMs: number): string {
  const date = new Date(epochMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}`;
}

/** 逻辑日起点（与 domain DEFAULT_DAY_START_MINUTES 对齐：凌晨 4 点）。 */
export const DAY_START_MINUTES = 240;

/** 某个日历日所属逻辑日的毫秒窗口：[当日 04:00, 次日 04:00)。 */
export function dayWindowOf(date: Date): { from: number; to: number } {
  const from =
    new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime() +
    DAY_START_MINUTES * 60_000;
  return { from, to: from + 86_400_000 };
}

const pad2 = (value: number) => String(value).padStart(2, "0");

/** 时间戳所属逻辑日 key（YYYY-MM-DD，按 04:00 边界）。 */
export function dayKeyOf(epochMs: number): string {
  const shifted = new Date(epochMs - DAY_START_MINUTES * 60_000);
  return `${shifted.getFullYear()}-${pad2(shifted.getMonth() + 1)}-${pad2(shifted.getDate())}`;
}

/** 逻辑日 key → 当日 Date（月份 0 起算已处理）。 */
export function parseDayKey(key: string): Date {
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day);
}

export interface DateHeading {
  /** “10月28日 周三”，标题栏用 */
  compact: string;
  /** “2026年10月28日 周三”，页头用 */
  full: string;
}

export function formatDateHeading(date = new Date()): DateHeading {
  const weekday = WEEKDAYS[date.getDay()];
  const monthDay = `${date.getMonth() + 1}月${date.getDate()}日`;
  return {
    compact: `${monthDay} ${weekday}`,
    full: `${date.getFullYear()}年${monthDay} ${weekday}`
  };
}
