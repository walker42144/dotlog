import { invoke } from "@tauri-apps/api/core";
import { dayKeyOf, dayWindowOf } from "./format";

// 与 docs/ipc-contract.md §2/§3 对齐的视图类型（Rust 侧为权威定义）。

export type Phase = "idle" | "recording";
export type PunchMode = "stop" | "chain";
export type PointSource = "button" | "hotkey" | "tray" | "second_instance" | "api";

export interface TagView {
  id: string;
  name: string;
  color: string | null;
}

export interface CategoryView {
  id: string;
  name: string;
  color: string;
  parentId: string;
}

export interface SegmentView {
  id: string;
  startAt: number;
  endAt: number | null;
  durationSeconds: number;
  title: string;
  note: string | null;
  category: CategoryView | null;
  tags: TagView[];
  isFocus: boolean;
  isIgnored: boolean;
  status: "active" | "closed";
  source: "manual" | "hotkey" | "tray" | "system" | "plugin" | "import";
  revision: number;
}

export interface TimerState {
  phase: Phase;
  active: SegmentView | null;
  elapsedSeconds: number;
  sessionStartedAt?: number | null;
  mode: PunchMode;
  paused: boolean;
}

export interface PendingNote {
  segmentId: string;
  startAt: number;
  endAt: number;
}

export interface PunchResult {
  closedSegment: SegmentView;
  newSegment: SegmentView | null;
  pendingNote: PendingNote | null;
  undoToken: string | null;
  shortSegment: boolean;
}

export interface DayStats {
  recordedSeconds: number;
  effectiveSeconds: number;
  blankSeconds: number;
  focusSeconds: number;
  coverageRate: number;
  switchCount: number;
  segmentCount: number;
  averageSegmentSeconds: number;
}

export interface GapView {
  startAt: number;
  endAt: number;
  seconds: number;
}

export interface DayView {
  date: string;
  windowStart: number;
  windowEnd: number;
  segments: SegmentView[];
  gaps: GapView[];
  stats: DayStats;
}

export interface UndoResult {
  restoredSegment: SegmentView | null;
  removedSegmentId: string | null;
}

export interface SegmentPatch {
  categoryId?: string | null;
  isFocus?: boolean;
  isIgnored?: boolean;
  tags?: string[];
  title?: string;
  note?: string;
  startAt?: number;
  endAt?: number;
}

export interface BackfillInput {
  categoryId?: string | null;
  isFocus?: boolean;
  isIgnored?: boolean;
  startAt: number;
  endAt: number;
  title?: string;
  note?: string;
}

export interface HistoryView {
  items: SegmentView[];
  total: number;
}

export interface HistoryQuery {
  from: number;
  to: number;
  text?: string;
  limit?: number;
  offset?: number;
}

export type Granularity = "day" | "week" | "month";
export type ExportFormat = "json" | "csv";

export interface CategorySeconds {
  categoryId: string | null;
  seconds: number;
}

export interface StatsBucket {
  dateStart: string;
  dateEnd: string;
  windowStart: number;
  windowEnd: number;
  recordedSeconds: number;
  effectiveSeconds: number;
  focusSeconds: number;
  blankSeconds: number;
  coverageRate: number;
  switchCount: number;
  segmentCount: number;
  byCategory: CategorySeconds[];
}

export interface StatsView {
  buckets: StatsBucket[];
}

export interface ExportResult {
  path: string;
  count: number;
}

export interface ImportRow {
  index: number;
  startAt: number;
  endAt: number;
  seconds: number;
  title: string;
  categoryName: string | null;
  status: "ok" | "conflict" | "invalid";
  reason: string | null;
  conflictIds: string[];
}

export interface ImportPreview {
  total: number;
  importable: number;
  conflicts: number;
  invalid: number;
  rows: ImportRow[];
}

export interface ImportResult {
  inserted: number;
  skipped: number;
}

export interface BackupEntry {
  path: string;
  createdAt: number;
  sizeBytes: number;
}

export type SettingsMap = Record<string, unknown>;

// ---- M5 集成（ipc-contract §3.5/§4） ----

export type IntegrationCapability =
  | "segments:read"
  | "tags:write"
  | "network"
  | "clipboard:read"
  | "ui:panel"
  | "secret:store";

export interface Memo { name: string; content: string; visibility: string; createTime: string }
export interface MemoPage { user: string; userId: string; instanceUrl: string; memos: Memo[]; nextPageToken: string | null; apiVersion: string }

export interface IntegrationView {
  id: string;
  name: string;
  version: string;
  enabled: boolean;
  installed?: boolean;
  configured: boolean;
  capabilities: IntegrationCapability[];
  lastError: string | null;
  lastSyncAt: number | null;
}

export type OutboxState = "pending" | "inflight" | "done" | "failed";

export interface OutboxView {
  id: string;
  eventType: string;
  state: OutboxState;
  attempts: number;
  nextRetryAtMs: number | null;
  lastError: string | null;
  createdAtMs: number;
  updatedAtMs: number;
  title: string | null;
}

/** AI 分类建议；只存在于内存，确认后才落库（plugin-sdk §8.2）。 */
export interface AiSuggestion {
  revision: number;
  original?: SegmentView;
  segmentId: string;
  title: string | null;
  tags: string[];
  categoryId: string | null;
  reason: string;
}

export interface AiReviewResult {
  markdown: string; recordCount: number; recordedSeconds: number; from: string; to: string; model: string;
}

export interface IntegrationTestResult {
  ok: boolean;
  message: string;
}

export interface AiApplyInput {
  segmentId: string;
  revision: number;
  title?: string | null;
  /** 空串表示清空分类。 */
  categoryId?: string | null;
  tags?: string[];
}

// ---------- 番茄Todo 集成 ----------

export interface TomatodoFocusPreview {
  id: number;
  name: string;
  startAtMs: number;
  endAtMs: number;
  durationMinutes: number;
  startText: string;
  endText: string;
}

export interface TomatodoScanResult {
  dbPath: string;
  fileMtimeMs: number;
  totalRecords: number;
  completedRecords: number;
  totalMinutes: number;
  earliestStartMs: number | null;
  latestEndMs: number | null;
  todoCount: number;
  preview: TomatodoFocusPreview[];
}

export interface TomatodoImportResult {
  imported: number;
  skippedExisting: number;
  skippedIncomplete: number;
  conflicts: number;
  errors: string[];
}

export interface TomatodoDotlogSegment {
  id: string;
  startAtMs: number;
  endAtMs: number;
  title: string;
  categoryName: string | null;
  tags: string[];
}

export interface TomatodoComparePair {
  tomatodo: TomatodoFocusPreview | null;
  dotlogSegments: TomatodoDotlogSegment[];
  overlapSeconds: number;
  tomatodoSeconds: number;
  dotlogSeconds: number;
  coveragePercent: number;
}

export interface TomatodoCompareSummary {
  tomatodoMinutes: number;
  dotlogMinutes: number;
  missingInDotlog: number;
  extraInDotlog: number;
  matchedPairs: number;
}

export interface TomatodoCompareResult {
  summary: TomatodoCompareSummary;
  pairs: TomatodoComparePair[];
}

export interface TomatodoAnalysis {
  pairIndex: number;
  tomatodoSubject: string;
  dotlogSummary: string;
  suggestion: string;
  reason: string;
}

export type RecoveryReason = "cross_day" | "long_gap" | "clock_anomaly" | "orphan_active";
export type RecoveryAction = "truncate" | "extend" | "discard" | "keep";

/** `recovery_get` 的返回（ipc-contract §2）。 */
export interface RecoveryInfo {
  required: boolean;
  reason: RecoveryReason | null;
  activeSegment: SegmentView | null;
  detectedAt: number;
  options: RecoveryAction[];
}

export interface DiagnosticsSnapshot {
  path: string;
}

/** E_OVERLAP 的 detail.conflicts 项（与 Rust 侧序列化结构一致）。 */
export interface ConflictView {
  id: string;
  startAt: number;
  endAt: number | null;
  title: string;
}

export interface IpcError {
  code: string;
  message: string;
  detail?: { conflicts?: ConflictView[] };
}

export const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function makeError(code: string, message: string): IpcError {
  return { code, message };
}

export function toIpcError(error: unknown): IpcError {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof (error as { code: unknown }).code === "string"
  ) {
    return error as IpcError;
  }
  if (error instanceof Error) {
    return makeError("E_UNKNOWN", error.message);
  }
  return makeError("E_UNKNOWN", String(error));
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!IS_TAURI) {
    return mockCall<T>(command, args);
  }
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toIpcError(error);
  }
}

export const ipc = {
  getState: () => call<TimerState>("timer_get_state"),
  start: (source: PointSource = "button") => call<TimerState>("timer_start", { source }),
  punch: (source: PointSource = "button") => call<PunchResult>("timer_punch", { source }),
  pause: (source: PointSource = "button") => call<PunchResult>("timer_pause", { source }),
  finish: (source: PointSource = "button") => call<PunchResult>("timer_finish", { source }),
  undo: () => call<UndoResult>("timer_undo"),
  dayToday: () => call<DayView>("day_today"),
  dayGet: (date: string) => call<DayView>("day_get", { date }),
  getPendingNote: () => call<SegmentView | null>("pending_note_get"),
  confirmSession: (id: string) => call<void>("segment_confirm", { id }),
  segmentGet: (id: string) => call<SegmentView>("segment_get", { id }),
  updateSegment: (id: string, revision: number, patch: SegmentPatch) =>
    call<SegmentView>("segment_update", { id, revision, patch }),
  backfillSegment: (input: BackfillInput) => call<SegmentView>("segment_backfill", { input }),
  deleteSegment: (id: string) => call<void>("segment_delete", { id }),
  splitSegment: (id: string, at: number) => call<SegmentView[]>("segment_split", { id, at }),
  mergeSegments: (ids: string[]) => call<SegmentView>("segment_merge", { ids }),
  historyQuery: (query: HistoryQuery) =>
    call<HistoryView>("history_query", {
      from: query.from,
      to: query.to,
      text: query.text ?? null,
      limit: query.limit ?? 50,
      offset: query.offset ?? 0
    }),
  statsGet: (from: number, to: number, granularity: Granularity) =>
    call<StatsView>("stats_get", { from, to, granularity }),
  categories: () => call<CategoryView[]>("categories_list"),
  settingsGet: (scope?: string) => call<SettingsMap>("settings_get", { scope: scope ?? null }),
  settingsPatch: (scope: string, entries: Array<{ key: string; value: unknown }>) =>
    call<SettingsMap>("settings_patch", { scope, entries }),
  settingsReset: (scope: string, key?: string) =>
    call<void>("settings_reset", { scope, key: key ?? null }),
  exportData: (format: ExportFormat, from: number, to: number, path?: string) =>
    call<ExportResult>("export_data", { format, from, to, path: path ?? null }),
  importPreview: (content: string) => call<ImportPreview>("import_preview", { content }),
  importCommit: (content: string) => call<ImportResult>("import_commit", { content }),
  dataOpenFolder: (kind: "data" | "backups" | "exports" | "restore-safety") => IS_TAURI ? call<void>("data_open_folder", { kind }) : Promise.resolve(),
  backupNow: () => call<BackupEntry>("backup_now"),
  backupList: () => call<BackupEntry[]>("backup_list"),
  backupRestore: (path: string) => call<void>("backup_restore", { path }),
  recoveryGet: () => call<RecoveryInfo>("recovery_get"),
  recoveryResolve: (action: RecoveryAction) =>
    call<TimerState>("recovery_resolve", { action }),
  diagnosticsSnapshot: () => call<DiagnosticsSnapshot>("diagnostics_snapshot"),
  integrationList: () => call<IntegrationView[]>("integration_list"),
  memosBrowse: (pageToken?: string) => call<MemoPage>("memos_browse", { pageToken: pageToken ?? null }),
  memosSave: (content: string, visibility: string, name?: string) => call<Memo>("memos_save", { content, visibility, name: name ?? null }),
  memosDelete: (name: string) => call<void>("memos_delete", { name }),
  integrationSetInstalled: (id: string, installed: boolean) =>
    call<IntegrationView>("integration_set_installed", { id, installed }),
  integrationSetEnabled: (id: string, enabled: boolean) =>
    call<IntegrationView>("integration_set_enabled", { id, enabled }),
  integrationConfigSet: (id: string, config: Record<string, unknown>) =>
    call<IntegrationView>("integration_config_set", { id, config }),
  integrationSecretSet: (id: string, secret: string) =>
    call<void>("integration_secret_set", { id, secret }),
  integrationTest: (id: string) => call<IntegrationTestResult>("integration_test", { id }),
  outboxList: (state?: OutboxState) => call<OutboxView[]>("outbox_list", { state: state ?? null }),
  outboxRetry: (id: string) => call<OutboxView>("outbox_retry", { id }),
  aiSuggest: (date?: string) => call<AiSuggestion[]>("ai_suggest", { date: date ?? null }),
  aiModels: () => call<string[]>("ai_models"),
  aiReview: (input: {from:string;to:string;question:string}) => call<AiReviewResult>("ai_review", {input}),
  aiApply: (input: AiApplyInput) => call<SegmentView>("ai_apply", { input }),
  tomatodoScan: () => call<TomatodoScanResult>("tomatodo_scan"),
  tomatodoImport: () => call<TomatodoImportResult>("tomatodo_import"),
  tomatodoCompare: () => call<TomatodoCompareResult>("tomatodo_compare"),
  tomatodoAnalyze: () => call<TomatodoAnalysis[]>("tomatodo_analyze")
};

export function friendlyMessage(error: IpcError): string {
  switch (error.code) {
    case "E_INVALID_RANGE":
      return error.message || "时间范围不合法，请检查起止时间";
    case "E_NOT_RECORDING":
      return "当前没有进行中的记录";
    case "E_ACTIVE_EXISTS":
      return "已经有一段记录在进行中";
    case "E_DB_BUSY":
      return "数据库正忙，请重试一次";
    case "E_UNDO_EXPIRED":
      return "已超过撤销时限，请到时间线里修改";
    case "E_UNDO_UNAVAILABLE":
      return "最近一次操作已无法撤销";
    case "E_SEGMENT_NOT_FOUND":
      return "这条记录已不存在，已重新加载";
    case "E_STALE_REVISION":
      return "记录已更新，请重新打开补记";
    case "E_OVERLAP":
      return "时间段与已有记录重叠";
    case "E_SETTING_INVALID":
      return "设置值不合法，请检查后重试";
    case "E_PLUGIN_DISABLED":
      return "该集成未启用，请先在集成页打开";
    case "E_INTEGRATION_NOT_CONFIGURED":
      return "请先完成该集成的配置";
    case "E_NETWORK":
      return "网络请求失败，请检查地址与连接";
    case "E_BACKUP_FAILED":
      return "备份失败，请稍后重试";
    case "E_RESTORE_INVALID":
      return error.message || "备份文件不可用，恢复已取消";
    case "E_CLOCK_ANOMALY":
      return "系统时间似乎发生了变化，请先确认进行中的记录";
    case "E_DB_CORRUPT":
      return "数据文件校验未通过，建议从备份恢复";
    default:
      return error.message || "操作失败，请重试";
  }
}

// ---- 浏览器预览用的内存模拟（仅在非 Tauri 环境生效，便于界面走查） ----

type MockPointKind = "start" | "resume" | "punch" | "pause" | "finish" | "edit" | "backfill" | "delete";

/** 编辑类操作的撤销凭据（对齐 domain EditUndo）。 */
interface MockEditUndo {
  restore: SegmentView[];
  removeIds: string[];
}

interface MockPoint {
  id: string;
  kind: MockPointKind;
  at: number;
  segmentId?: string;
  undone: boolean;
  edit?: MockEditUndo;
  pausedFinish?: boolean;
}

let mockActive: SegmentView | null = null;
let mockClosed: SegmentView[] = [];
let mockPoints: MockPoint[] = [];
let mockPaused = false;
let mockSessionIds: string[] = [];
const mockSessionGroups: Record<string, string[]> = {};
let mockCounter = 0;

const UNDO_WINDOW_MS = 15_000;
const DAY_START_MINUTES = 240;
const DAY_MS = 86_400_000;

/** 与 `dotlog-application::settings::default_settings` 对齐的预览默认值。 */
const DEFAULT_SETTINGS: SettingsMap = {
  "punch.mode": "stop",
  "punch.undo_window_ms": 15_000,
  "punch.short_segment_ms": 15_000,
  "day.start_minutes": 240,
  "day.active_window": { startMinutes: 240, endMinutes: 1680 },
  "focus.min_minutes": 10,
  "hotkey.punch": "Ctrl+Shift+Space",
  "notify.on_punch": true,
  "launch.at_login": false,
  "ui.theme": "system",
  "ui.motion": "system",
  "backup.enabled": true,
  "backup.keep_daily": 7,
  "backup.keep_weekly": 4,
  "backup.keep_monthly": 6
};

let mockSettings: SettingsMap = { ...DEFAULT_SETTINGS };

/** 与迁移 0002 的 8 条分类种子对齐。 */
const MOCK_CATEGORIES: CategoryView[] = [
  { id: "cat-study", name: "学习", color: "#2563eb", parentId: "" },
  { id: "cat-work", name: "工作", color: "#0f766e", parentId: "" },
  { id: "cat-life", name: "生活", color: "#b45309", parentId: "" },
  { id: "cat-sport", name: "运动", color: "#15803d", parentId: "" },
  { id: "cat-reading", name: "阅读", color: "#7c3aed", parentId: "" },
  { id: "cat-fun", name: "娱乐", color: "#be185d", parentId: "" },
  { id: "cat-sleep", name: "睡眠", color: "#475569", parentId: "" },
  { id: "cat-other", name: "其它", color: "#6b7280", parentId: "" }
];

let mockBackups: BackupEntry[] = [
  {
    path: "C:\\Users\\preview\\AppData\\Roaming\\dotlog\\backups\\dotlog-20261028-090000.db",
    createdAt: Date.now() - DAY_MS,
    sizeBytes: 184_320
  },
  {
    path: "C:\\Users\\preview\\AppData\\Roaming\\dotlog\\backups\\dotlog-20261027-090000.db",
    createdAt: Date.now() - 2 * DAY_MS,
    sizeBytes: 180_224
  }
];

function nextId(prefix: string): string {
  mockCounter += 1;
  return `${prefix}-${Date.now()}-${mockCounter}`;
}

/** 预览用示例段：让分析页与时间线在浏览器里即可看到真实布局。 */
function seedSegments(): SegmentView[] {
  const now = Date.now();
  const base = new Date();
  base.setHours(0, 0, 0, 0);
  const slots: Array<{
    dayOffset: number;
    hour: number;
    minute: number;
    minutes: number;
    title: string;
    note?: string;
    category: CategoryView;
    focus?: boolean;
  }> = [
    { dayOffset: -1, hour: 9, minute: 10, minutes: 95, title: "整理周报", category: MOCK_CATEGORIES[1], focus: true },
    { dayOffset: -1, hour: 11, minute: 0, minutes: 45, title: "读《深度工作》", category: MOCK_CATEGORIES[4] },
    { dayOffset: -1, hour: 19, minute: 30, minutes: 60, title: "跑步", category: MOCK_CATEGORIES[3] },
    { dayOffset: 0, hour: 8, minute: 30, minutes: 50, title: "英语精读", category: MOCK_CATEGORIES[0], focus: true },
    { dayOffset: 0, hour: 9, minute: 40, minutes: 140, title: "需求评审与实现", note: "M3 统计口径", category: MOCK_CATEGORIES[1], focus: true },
    { dayOffset: 0, hour: 13, minute: 20, minutes: 40, title: "午饭与散步", category: MOCK_CATEGORIES[2] }
  ];
  const segments: SegmentView[] = [];
  for (const slot of slots) {
    const startAt =
      base.getTime() + slot.dayOffset * DAY_MS + (slot.hour * 60 + slot.minute) * 60_000;
    const endAt = startAt + slot.minutes * 60_000;
    if (startAt >= now || endAt > now) continue;
    segments.push({
      id: nextId("preview"),
      startAt,
      endAt,
      durationSeconds: Math.floor((endAt - startAt) / 1000),
      title: slot.title,
      note: slot.note ?? null,
      category: slot.category,
      tags: [],
      isFocus: slot.focus ?? false,
      isIgnored: false,
      status: "closed",
      source: "manual",
      revision: 1
    });
  }
  return segments;
}

if (!IS_TAURI) {
  mockClosed = seedSegments();
}

/**
 * 浏览器走查用：`?recovery=long_gap` 等参数模拟启动恢复提示，
 * 便于截图检查恢复对话框的深浅色表现。
 */
const RECOVERY_REASONS: RecoveryReason[] = [
  "cross_day",
  "long_gap",
  "clock_anomaly",
  "orphan_active"
];

let mockRecovery: RecoveryInfo | null = null;
if (!IS_TAURI) {
  const requested = new URLSearchParams(window.location.search).get("recovery");
  if (requested !== null) {
    const reason = RECOVERY_REASONS.includes(requested as RecoveryReason)
      ? (requested as RecoveryReason)
      : "cross_day";
    mockActive = mockSegment(Date.now() - 3 * 3_600_000);
    mockRecovery = {
      required: true,
      reason,
      activeSegment: mockActive,
      detectedAt: Date.now(),
      options:
        reason === "cross_day"
          ? ["truncate", "extend", "discard"]
          : reason === "clock_anomaly"
            ? ["extend", "keep"]
            : reason === "long_gap"
              ? ["truncate", "extend", "discard"]
              : ["extend", "discard", "keep"]
    };
  }
}

function mockSegment(startAt: number): SegmentView {
  return {
    id: nextId("preview"),
    startAt,
    endAt: null,
    durationSeconds: 0,
    title: "",
    note: null,
    category: null,
    tags: [],
    isFocus: false,
    isIgnored: false,
    status: "active",
    source: "manual",
    revision: 1
  };
}

function mockState(): TimerState {
  const active = mockActive;
  const session = [...mockClosed, ...(active ? [active] : [])].filter((s) => mockSessionIds.includes(s.id));
  const elapsedSeconds = session.reduce((sum, s) => sum + Math.max(0, Math.floor(((s.endAt ?? Date.now()) - s.startAt) / 1000)), 0);
  return {
    phase: active ? "recording" : "idle",
    active: active ? { ...active, durationSeconds: elapsedSeconds } : null,
    elapsedSeconds,
    sessionStartedAt: session.length ? Math.min(...session.map((s) => s.startAt)) : null,
    mode: mockSettings["punch.mode"] === "chain" ? "chain" : "stop",
    paused: mockPaused
  };
}

function pushPoint(kind: MockPointKind, at: number, segmentId?: string, edit?: MockEditUndo): void {
  mockPoints.push({ id: nextId("pt"), kind, at, segmentId, undone: false, edit });
}

function mockClose(at = Date.now()): SegmentView {
  if (!mockActive) {
    throw makeError("E_NOT_RECORDING", "当前没有进行中的记录");
  }
  if (at <= mockActive.startAt) {
    throw makeError("E_INVALID_RANGE", "时间范围非法");
  }
  const closed: SegmentView = {
    ...mockActive,
    endAt: at,
    durationSeconds: Math.floor((at - mockActive.startAt) / 1000),
    status: "closed",
    revision: mockActive.revision + 1
  };
  mockActive = null;
  mockClosed = [...mockClosed, closed];
  return closed;
}

function mockResult(closed: SegmentView): PunchResult {
  const slices = mockClosed.filter((s) => (mockSessionGroups[closed.id] ?? [closed.id]).includes(s.id));
  const logicalClosed = { ...closed, startAt: Math.min(closed.startAt, ...slices.map((s) => s.startAt)), durationSeconds: slices.reduce((sum, s) => sum + s.durationSeconds, 0) || closed.durationSeconds };
  return {
    closedSegment: logicalClosed,
    newSegment: null,
    pendingNote: {
      segmentId: closed.id,
      startAt: closed.startAt,
      endAt: closed.endAt ?? closed.startAt
    },
    undoToken: `preview-${closed.id}`,
    shortSegment: closed.durationSeconds * 1000 < Number(mockSettings["punch.short_segment_ms"] ?? 15_000)
  };
}

function lastLivePoint(): MockPoint | null {
  for (let index = mockPoints.length - 1; index >= 0; index -= 1) {
    const point = mockPoints[index];
    if (!point.undone) return point;
  }
  return null;
}

function mockUndo(): UndoResult {
  const point = lastLivePoint();
  if (!point) throw makeError("E_UNDO_UNAVAILABLE", "没有可撤销的操作");
  if (Date.now() - point.at > Number(mockSettings["punch.undo_window_ms"] ?? UNDO_WINDOW_MS)) {
    throw makeError("E_UNDO_EXPIRED", "撤销窗口已过");
  }

  point.undone = true;
  if (point.pausedFinish && point.segmentId) {
    mockSessionIds = mockSessionGroups[point.segmentId] ?? [point.segmentId]; mockPaused = true;
    return { restoredSegment: null, removedSegmentId: null };
  }
  if (point.kind === "start" || point.kind === "resume") {
    const removed = mockActive?.id ?? null;
    mockActive = null;
    mockPaused = point.kind === "resume";
    mockSessionIds = mockPaused ? (mockSessionGroups[point.segmentId ?? ""] ?? []).filter((id) => id !== removed) : [];
    return { restoredSegment: null, removedSegmentId: removed };
  }

  // 编辑类撤销：按凭据恢复快照、移除新建段。
  if (point.edit) {
    const { restore, removeIds } = point.edit;
    mockClosed = mockClosed.filter(
      (segment) => !removeIds.includes(segment.id) && !restore.some((r) => r.id === segment.id)
    );
    if (
      mockActive &&
      (removeIds.includes(mockActive.id) || restore.some((r) => r.id === mockActive!.id))
    ) {
      mockActive = null;
    }
    let restored: SegmentView | null = null;
    for (const snapshot of restore) {
      const revived: SegmentView = { ...snapshot, revision: snapshot.revision + 1 };
      if (snapshot.endAt === null) {
        mockActive = { ...revived, status: "active" };
        restored = mockActive;
      } else {
        mockClosed = [...mockClosed, revived];
        restored = revived;
      }
    }
    mockPaused = false;
    return { restoredSegment: restored, removedSegmentId: removeIds[0] ?? null };
  }

  const id = point.segmentId;
  if (!id) throw makeError("E_UNDO_UNAVAILABLE", "撤销信息不完整");
  const stored = mockClosed.find((segment) => segment.id === id);
  if (!stored) throw makeError("E_UNDO_UNAVAILABLE", "记录已不存在");

  mockSessionIds = mockSessionGroups[id] ?? [id];
  mockClosed = mockClosed.filter((segment) => segment.id !== id);
  mockActive = {
    ...stored,
    endAt: null,
    status: "active",
    revision: stored.revision + 1
  };
  mockPaused = false;
  return { restoredSegment: mockActive, removedSegmentId: null };
}

function mockDay(requestedDate?: string): DayView {
  const now = Date.now();
  const reference = requestedDate ? new Date(`${requestedDate}T12:00:00`).getTime() : now;
  const tzOffsetMin = -new Date().getTimezoneOffset();
  const tzMs = tzOffsetMin * 60_000;
  const dayStartMs = DAY_START_MINUTES * 60_000;
  const dayIndex = Math.floor((reference + tzMs - dayStartMs) / DAY_MS);
  const windowStart = dayIndex * DAY_MS + dayStartMs - tzMs;
  const windowEnd = windowStart + DAY_MS;
  const statsEnd = Math.min(windowEnd, Math.max(windowStart, now));

  const all = [...mockClosed, ...(mockActive ? [mockActive] : [])];
  const segments: SegmentView[] = [];
  const clips: Array<{ startAt: number; endAt: number }> = [];

  for (const segment of all) {
    const end = segment.endAt ?? now;
    const start = Math.max(segment.startAt, windowStart);
    const clippedEnd = Math.min(end, statsEnd);
    if (clippedEnd <= start) continue;
    segments.push({
      ...segment,
      startAt: start,
      endAt: segment.status === "active" ? null : clippedEnd,
      durationSeconds: Math.floor((clippedEnd - start) / 1000)
    });
    clips.push({ startAt: start, endAt: clippedEnd });
  }

  clips.sort((left, right) => left.startAt - right.startAt);
  const gaps: GapView[] = [];
  let cursor = windowStart;
  for (const clip of clips) {
    if (clip.startAt > cursor) {
      gaps.push({
        startAt: cursor,
        endAt: Math.min(clip.startAt, statsEnd),
        seconds: Math.floor((Math.min(clip.startAt, statsEnd) - cursor) / 1000)
      });
    }
    cursor = Math.max(cursor, clip.endAt);
  }
  if (cursor < statsEnd) {
    gaps.push({
      startAt: cursor,
      endAt: statsEnd,
      seconds: Math.floor((statsEnd - cursor) / 1000)
    });
  }

  const recordedSeconds = segments.reduce((sum, segment) => sum + segment.durationSeconds, 0);
  const effectiveSeconds = segments
    .filter((segment) => segment.title.length > 0)
    .reduce((sum, segment) => sum + segment.durationSeconds, 0);
  const windowSeconds = Math.floor((statsEnd - windowStart) / 1000);

  const date = new Date(windowStart);
  const pad = (value: number) => String(value).padStart(2, "0");

  return {
    date: `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
    windowStart,
    windowEnd,
    segments,
    gaps,
    stats: {
      recordedSeconds,
      effectiveSeconds,
      blankSeconds: Math.max(0, windowSeconds - recordedSeconds),
      focusSeconds: 0,
      coverageRate:
        windowSeconds > 0 ? Math.min(1, effectiveSeconds / windowSeconds) : 0,
      switchCount: Math.max(0, segments.length - 1),
      segmentCount: segments.length,
      averageSegmentSeconds:
        segments.length > 0 ? Math.floor(recordedSeconds / segments.length) : 0
    }
  };
}

function mockUpdateSegment(args?: Record<string, unknown>): SegmentView {
  const id = String(args?.id ?? "");
  const revision = Number(args?.revision ?? 0);
  const patch = (args?.patch ?? {}) as SegmentPatch;

  const target = mockActive?.id === id ? mockActive : mockClosed.find((s) => s.id === id);
  if (!target) throw makeError("E_SEGMENT_NOT_FOUND", "这条记录不存在");
  if (target.revision !== revision) {
    throw makeError("E_STALE_REVISION", "记录已被修改");
  }

  const newStart = patch.startAt !== undefined ? patch.startAt : target.startAt;
  const newEnd = patch.endAt !== undefined ? patch.endAt : target.endAt;
  if (patch.startAt !== undefined || patch.endAt !== undefined) {
    if (newStart >= Date.now()) throw makeError("E_INVALID_RANGE", "开始时间不能在未来");
    if (target.status === "active" && patch.endAt !== undefined) {
      throw makeError("E_INVALID_RANGE", "进行中的记录由打点结束，不能手动设置终点");
    }
    if (newEnd !== null && newEnd <= newStart) {
      throw makeError("E_INVALID_RANGE", "结束时间必须晚于开始时间");
    }
    const conflicts = mockConflicts(newStart, newEnd ?? Date.now(), id);
    if (conflicts.length > 0) throw overlapError(conflicts);
  }

  const updated: SegmentView = {
    ...target,
    title: patch.title !== undefined ? patch.title.trim() : target.title,
    note: patch.note !== undefined ? patch.note.trim() || null : target.note,
    startAt: newStart,
    endAt: newEnd,
    durationSeconds:
      newEnd !== null
        ? Math.floor((newEnd - newStart) / 1000)
        : Math.floor((Date.now() - newStart) / 1000),
    revision: target.revision + 1
  };
  if (patch.categoryId !== undefined) updated.category = MOCK_CATEGORIES.find((c) => c.id === patch.categoryId) ?? null;
  if (patch.isFocus !== undefined) updated.isFocus = patch.isFocus;
  if (patch.isIgnored !== undefined) updated.isIgnored = patch.isIgnored;
  if (patch.tags !== undefined) updated.tags = patch.tags.map((name) => ({ id: name, name, color: null }));


  if (mockActive?.id === id) {
    mockActive = updated;
  } else {
    mockClosed = mockClosed.map((segment) => (segment.id === id ? updated : segment));
  }
  pushPoint("edit", Date.now(), id, { restore: [target], removeIds: [] });
  return updated;
}

function mockAllSegments(): SegmentView[] {
  return [...mockClosed, ...(mockActive ? [mockActive] : [])];
}

function mockConflicts(start: number, end: number, excludeId?: string): ConflictView[] {
  return mockAllSegments()
    .filter((segment) => segment.id !== excludeId)
    .filter((segment) => segment.startAt < end && (segment.endAt ?? Date.now()) > start)
    .map((segment) => ({
      id: segment.id,
      startAt: segment.startAt,
      endAt: segment.endAt,
      title: segment.title
    }));
}

function overlapError(conflicts: ConflictView[]): IpcError {
  return { code: "E_OVERLAP", message: "时间段与已有记录重叠", detail: { conflicts } };
}

function mockBackfill(input: BackfillInput): SegmentView {
  const now = Date.now();
  if (input.endAt <= input.startAt) throw makeError("E_INVALID_RANGE", "结束时间必须晚于开始时间");
  if (input.endAt - input.startAt > DAY_MS) {
    throw makeError("E_INVALID_RANGE", "单次补录不能超过 24 小时");
  }
  if (input.endAt > now) throw makeError("E_INVALID_RANGE", "不能补录未来的时间");
  const conflicts = mockConflicts(input.startAt, input.endAt);
  if (conflicts.length > 0) throw overlapError(conflicts);

  const segment: SegmentView = {
    id: nextId("preview"),
    startAt: input.startAt,
    endAt: input.endAt,
    durationSeconds: Math.floor((input.endAt - input.startAt) / 1000),
    title: (input.title ?? "").trim(),
    note: (input.note ?? "").trim() || null,
    category: null,
    tags: [],
    isFocus: false,
    isIgnored: false,
    status: "closed",
    source: "manual",
    revision: 1
  };
  mockClosed = [...mockClosed, segment];
  pushPoint("backfill", now, segment.id, { restore: [], removeIds: [segment.id] });
  return segment;
}

function mockDelete(id: string): void {
  const now = Date.now();
  if (mockActive?.id === id) {
    const original = mockActive;
    mockActive = null;
    mockPaused = false;
    pushPoint("delete", now, id, { restore: [original], removeIds: [] });
    return;
  }
  const target = mockClosed.find((segment) => segment.id === id);
  if (!target) throw makeError("E_SEGMENT_NOT_FOUND", "这条记录不存在");
  mockClosed = mockClosed.filter((segment) => segment.id !== id);
  pushPoint("delete", now, id, { restore: [target], removeIds: [] });
}

function mockSplit(id: string, at: number): SegmentView[] {
  const isActive = mockActive?.id === id;
  const target = isActive ? mockActive : mockClosed.find((segment) => segment.id === id);
  if (!target) throw makeError("E_SEGMENT_NOT_FOUND", "这条记录不存在");
  const end = target.endAt ?? Date.now();
  if (at <= target.startAt || at >= end) {
    throw makeError("E_INVALID_RANGE", "拆分点必须落在记录内部");
  }

  const first: SegmentView = {
    ...target,
    endAt: at,
    status: "closed",
    durationSeconds: Math.floor((at - target.startAt) / 1000),
    revision: target.revision + 1
  };
  const second: SegmentView = {
    ...target,
    id: nextId("preview"),
    startAt: at,
    endAt: isActive ? null : target.endAt,
    status: isActive ? "active" : "closed",
    durationSeconds: isActive ? 0 : Math.floor((end - at) / 1000),
    revision: 1
  };

  if (isActive) {
    mockClosed = [...mockClosed, first];
    mockActive = second;
  } else {
    mockClosed = [...mockClosed.filter((segment) => segment.id !== id), first, second];
  }
  pushPoint("edit", Date.now(), id, { restore: [target], removeIds: [second.id] });
  return [first, second];
}

function mockMerge(ids: string[]): SegmentView {
  if (ids.length < 2) throw makeError("E_INVALID_RANGE", "合并至少需要两段记录");
  const targets = ids
    .map((id) => {
      const found = mockClosed.find((segment) => segment.id === id);
      if (!found) throw makeError("E_SEGMENT_NOT_FOUND", "记录不存在或仍在进行中");
      return found;
    })
    .sort((left, right) => left.startAt - right.startAt);

  const first = targets[0];
  const last = targets[targets.length - 1];
  const titles = [...new Set(targets.map((segment) => segment.title).filter(Boolean))];
  const notes = [...new Set(targets.map((segment) => segment.note ?? "").filter(Boolean))];
  const merged: SegmentView = {
    ...first,
    id: nextId("preview"),
    startAt: first.startAt,
    endAt: last.endAt,
    durationSeconds: Math.floor(((last.endAt ?? first.startAt) - first.startAt) / 1000),
    title: titles.join("、"),
    note: notes.join("\n") || null,
    revision: 1
  };
  mockClosed = [...mockClosed.filter((segment) => !ids.includes(segment.id)), merged];
  pushPoint("edit", Date.now(), merged.id, { restore: targets, removeIds: [merged.id] });
  return merged;
}

function mockHistory(args?: Record<string, unknown>): HistoryView {
  const from = Number(args?.from ?? 0);
  const to = Number(args?.to ?? 0);
  const rawText = typeof args?.text === "string" ? args.text.trim() : "";
  const limit = Number(args?.limit ?? 50);
  const offset = Number(args?.offset ?? 0);

  const matched = mockAllSegments()
    .filter((segment) => segment.startAt < to && (segment.endAt ?? Date.now()) > from)
    .filter(
      (segment) =>
        rawText === "" || segment.title.includes(rawText) || (segment.note ?? "").includes(rawText)
    )
    .sort((left, right) => right.startAt - left.startAt);
  return { items: matched.slice(offset, offset + limit), total: matched.length };
}

function mockWindows(from: number, to: number, granularity: Granularity): Array<[number, number]> {
  const windows: Array<[number, number]> = [];
  let cursor = from;
  while (cursor < to) {
    let next: number;
    if (granularity === "day") {
      next = cursor + DAY_MS;
    } else if (granularity === "week") {
      next = cursor + 7 * DAY_MS;
    } else {
      const date = new Date(cursor);
      next = new Date(date.getFullYear(), date.getMonth() + 1, 1).getTime();
    }
    const end = Math.min(next, to);
    if (end <= cursor) break;
    windows.push([cursor, end]);
    cursor = end;
  }
  return windows;
}

function mockStats(args?: Record<string, unknown>): StatsView {
  const from = Number(args?.from ?? 0);
  const to = Number(args?.to ?? 0);
  const granularity = (args?.granularity ?? "day") as Granularity;
  const now = Date.now();
  const focusMinutes = Number(mockSettings["focus.min_minutes"] ?? 10);
  const segments = mockAllSegments();

  const buckets: StatsBucket[] = [];
  for (const [windowStart, windowEnd] of mockWindows(from, to, granularity)) {
    const cappedEnd = Math.min(windowEnd, now);
    if (cappedEnd <= windowStart) continue;
    let recorded = 0;
    let effective = 0;
    let focus = 0;
    let count = 0;
    let effectiveCount = 0;
    const byCategory = new Map<string | null, number>();
    for (const segment of segments) {
      const end = segment.endAt ?? now;
      const start = Math.max(segment.startAt, windowStart);
      const clipEnd = Math.min(end, cappedEnd);
      if (clipEnd <= start) continue;
      const seconds = Math.floor((clipEnd - start) / 1000);
      count += 1;
      recorded += seconds;
      if (!segment.isIgnored && (segment.title.length > 0 || segment.category !== null)) {
        effective += seconds;
        effectiveCount += 1;
      }
      if (segment.isFocus && Math.floor((end - segment.startAt) / 1000) >= focusMinutes * 60) {
        focus += seconds;
      }
      if (!segment.isIgnored) {
        const key = segment.category?.id ?? null;
        byCategory.set(key, (byCategory.get(key) ?? 0) + seconds);
      }
    }
    const windowSeconds = Math.floor((cappedEnd - windowStart) / 1000);
    buckets.push({
      dateStart: dayKeyOf(windowStart),
      dateEnd: dayKeyOf(cappedEnd - 1),
      windowStart,
      windowEnd: cappedEnd,
      recordedSeconds: recorded,
      effectiveSeconds: effective,
      focusSeconds: focus,
      blankSeconds: Math.max(0, windowSeconds - recorded),
      coverageRate: windowSeconds > 0 ? Math.min(1, effective / windowSeconds) : 0,
      switchCount: Math.max(0, effectiveCount - 1),
      segmentCount: count,
      byCategory: [...byCategory.entries()]
        .map(([categoryId, seconds]) => ({ categoryId, seconds }))
        .filter((item) => item.seconds > 0)
        .sort((left, right) => right.seconds - left.seconds)
    });
  }
  return { buckets };
}

function mockSettingsGet(args?: Record<string, unknown>): SettingsMap {
  const scope = args?.scope;
  if (typeof scope === "string") {
    const id = scope.startsWith("plugin:") ? scope.slice("plugin:".length) : null;
    if (id === null) return scope === "app" ? { ...mockSettings } : {};
    return { ...(mockPluginConfig[id] ?? {}) };
  }
  return { ...mockSettings };
}

function mockSettingsPatch(args?: Record<string, unknown>): SettingsMap {
  const entries = (args?.entries as Array<{ key: string; value: unknown }>) ?? [];
  for (const entry of entries) mockSettings[entry.key] = entry.value;
  return mockSettingsGet(args);
}

function mockSettingsReset(args?: Record<string, unknown>): void {
  const key = args?.key;
  if (typeof key === "string" && key in DEFAULT_SETTINGS) {
    mockSettings[key] = DEFAULT_SETTINGS[key];
  } else if (typeof key !== "string") {
    mockSettings = { ...DEFAULT_SETTINGS };
  }
}

function mockExport(args?: Record<string, unknown>): ExportResult {
  const from = Number(args?.from ?? 0);
  const to = Number(args?.to ?? 0);
  const format = (args?.format ?? "csv") as ExportFormat;
  const count = mockAllSegments().filter(
    (segment) => segment.startAt < to && (segment.endAt ?? Date.now()) > from
  ).length;
  const date = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  const stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(
    date.getHours()
  )}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
  return {
    path: `C:\\Users\\preview\\AppData\\Roaming\\dotlog\\exports\\dotlog-${stamp}.${format}`,
    count
  };
}

function parseCsvLine(line: string): string[] {
  const cells: string[] = [];
  let cell = "";
  let quoted = false;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (quoted) {
      if (char === '"' && line[index + 1] === '"') {
        cell += '"';
        index += 1;
      } else if (char === '"') {
        quoted = false;
      } else {
        cell += char;
      }
    } else if (char === '"') {
      quoted = true;
    } else if (char === ",") {
      cells.push(cell);
      cell = "";
    } else {
      cell += char;
    }
  }
  cells.push(cell);
  return cells;
}

function analyzeMockImport(content: string): ImportPreview {
  const lines = content
    .replace(/^\uFEFF/, "")
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "");
  const rows: ImportRow[] = [];
  if (lines.length === 0) {
    return { total: 0, importable: 0, conflicts: 0, invalid: 0, rows };
  }

  const header = parseCsvLine(lines[0]).map((cell) => cell.trim().toLowerCase());
  const column = (name: string) => header.indexOf(name);
  const startIndex = column("start");
  const endIndex = column("end");
  if (startIndex < 0 || endIndex < 0) {
    throw makeError("E_INVALID_RANGE", "CSV 缺少 start / end 列");
  }
  const titleIndex = column("title");
  const categoryIndex = column("category");
  const accepted: Array<{ startAt: number; endAt: number }> = [];

  for (let index = 1; index < lines.length; index += 1) {
    const cells = parseCsvLine(lines[index]);
    const startAt = Date.parse(cells[startIndex] ?? "");
    const endAt = Date.parse(cells[endIndex] ?? "");
    const title = titleIndex >= 0 ? (cells[titleIndex] ?? "").trim() : "";
    const categoryName = categoryIndex >= 0 ? (cells[categoryIndex] ?? "").trim() || null : null;
    if (Number.isNaN(startAt) || Number.isNaN(endAt) || endAt <= startAt) {
      rows.push({
        index,
        startAt: Number.isNaN(startAt) ? 0 : startAt,
        endAt: Number.isNaN(endAt) ? 0 : endAt,
        seconds: 0,
        title,
        categoryName,
        status: "invalid",
        reason: "时间无法解析或终点早于起点",
        conflictIds: []
      });
      continue;
    }
    const conflicts = mockConflicts(startAt, endAt);
    const prior = accepted.some((row) => row.startAt < endAt && row.endAt > startAt);
    if (conflicts.length > 0 || prior) {
      rows.push({
        index,
        startAt,
        endAt,
        seconds: Math.floor((endAt - startAt) / 1000),
        title,
        categoryName,
        status: "conflict",
        reason: "与已有记录或前序导入行重叠",
        conflictIds: conflicts.map((conflict) => conflict.id)
      });
      continue;
    }
    accepted.push({ startAt, endAt });
    rows.push({
      index,
      startAt,
      endAt,
      seconds: Math.floor((endAt - startAt) / 1000),
      title,
      categoryName,
      status: "ok",
      reason: null,
      conflictIds: []
    });
  }

  return {
    total: rows.length,
    importable: rows.filter((row) => row.status === "ok").length,
    conflicts: rows.filter((row) => row.status === "conflict").length,
    invalid: rows.filter((row) => row.status === "invalid").length,
    rows
  };
}

function mockImportCommit(args?: Record<string, unknown>): ImportResult {
  const preview = analyzeMockImport(String(args?.content ?? ""));
  let inserted = 0;
  for (const row of preview.rows) {
    if (row.status !== "ok") continue;
    mockClosed = [
      ...mockClosed,
      {
        id: nextId("preview"),
        startAt: row.startAt,
        endAt: row.endAt,
        durationSeconds: row.seconds,
        title: row.title,
        note: null,
        category: null,
        tags: [],
        isFocus: false,
        isIgnored: false,
        status: "closed",
        source: "import",
        revision: 1
      }
    ];
    inserted += 1;
  }
  return { inserted, skipped: preview.total - preview.importable };
}

function mockBackupNow(): BackupEntry {
  const created = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  const stamp = `${created.getFullYear()}${pad(created.getMonth() + 1)}${pad(
    created.getDate()
  )}-${pad(created.getHours())}${pad(created.getMinutes())}${pad(created.getSeconds())}`;
  const entry: BackupEntry = {
    path: `C:\\Users\\preview\\AppData\\Roaming\\dotlog\\backups\\dotlog-${stamp}.db`,
    createdAt: created.getTime(),
    sizeBytes: 186_368
  };
  mockBackups = [entry, ...mockBackups];
  return entry;
}

function mockBackupRestore(path: string): void {
  if (!mockBackups.some((entry) => entry.path === path)) {
    throw makeError("E_RESTORE_INVALID", "备份文件不可用");
  }
}

function mockRecoveryGet(): RecoveryInfo {
  return (
    mockRecovery ?? {
      required: false,
      reason: null,
      activeSegment: null,
      detectedAt: Date.now(),
      options: []
    }
  );
}

function mockResolveRecovery(action: RecoveryAction): TimerState {
  if (!mockRecovery) throw makeError("E_UNKNOWN", "没有需要处理的恢复项");
  const at = Date.now();

  if (action === "extend" && mockActive) {
    mockClose(at);
  } else if (action === "truncate" && mockActive) {
    const cutoff =
      mockRecovery.reason === "long_gap"
        ? at - 45 * 60_000
        : dayWindowOf(new Date()).from;
    mockClose(Math.max(mockActive.startAt + 60_000, Math.min(at, cutoff)));
  } else if (action === "discard" && mockActive) {
    const removed = mockActive.id;
    mockActive = null;
    mockClosed = mockClosed.filter((segment) => segment.id !== removed);
  }

  mockRecovery = null;
  mockPaused = false;
  return mockState();
}

function mockDiagnostics(): DiagnosticsSnapshot {
  const date = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  const stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(
    date.getHours()
  )}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
  return {
    path: `C:\\Users\\preview\\AppData\\Roaming\\dotlog\\diagnostics\\dotlog-diagnostics-${stamp}.json`
  };
}

// ---- M5 集成（浏览器走查模拟） ----

/** 插件设置（scope `plugin:<id>`）。 */
let mockMemos: Memo[] = [];

const mockPluginConfig: Record<string, Record<string, unknown>> = {
  memos: { instanceUrl: "https://memos.example.com", apiVersion: "v0.24" },
  ai: {
    provider: "ollama",
    baseUrl: "http://localhost:11434/v1",
    model: "qwen2.5:7b",
    privacyMode: "title_only"
  }
};

/** 插件启停状态。 */
const mockEnabled: Record<string, boolean> = { memos: true, ai: true };

/** 钥匙串模拟：`<id>:token`。 */
const mockSecrets: Record<string, string> = {};

const INTEGRATION_META: Array<{
  id: string;
  name: string;
  version: string;
  capabilities: IntegrationCapability[];
  description: string;
}> = [
  {
    id: "memos",
    name: "Memos",
    version: "0.1.0",
    capabilities: ["segments:read", "network", "secret:store", "ui:panel"],
    description: "记录关闭后自动发布为 Memos 日记，幂等重试，不打断打点。"
  },
  {
    id: "ai",
    name: "AI 助手",
    version: "0.1.0",
    capabilities: ["network", "secret:store"],
    description: "用本地或 OpenAI 兼容服务整理标题与分类，建议需逐条确认。"
  },
  {
    id: "tomatodo",
    name: "番茄Todo",
    version: "0.1.0",
    capabilities: ["ui:panel", "segments:read"],
    description: "读取本机番茄Todo 数据，把专注记录导入 DotLog 并对比分析。"
  }
];

function mockOutboxRow(
  eventType: string,
  state: OutboxState,
  title: string,
  minutesAgo: number,
  extra?: Partial<OutboxView>
): OutboxView {
  const updatedAtMs = Date.now() - minutesAgo * 60_000;
  return {
    id: nextId("obx"),
    eventType,
    state,
    attempts: state === "failed" ? 3 : 1,
    nextRetryAtMs: null,
    lastError: null,
    createdAtMs: updatedAtMs,
    updatedAtMs,
    title,
    ...extra
  };
}

let mockOutbox: OutboxView[] = [
  mockOutboxRow("memos.publish", "done", "需求评审与实现", 8, {
    lastError: null
  }),
  mockOutboxRow("memos.publish", "failed", "跑步", 22, {
    lastError: "HTTP 502: Bad Gateway",
    nextRetryAtMs: Date.now() + 4 * 60_000
  }),
  mockOutboxRow("memos.publish", "pending", "午饭与散步", 3)
];

function mockProviderStatus(provider: string): Pick<IntegrationView, "lastError" | "lastSyncAt"> {
  const failed = mockOutbox
    .filter((row) => row.eventType.startsWith(`${provider}.`) && row.lastError !== null)
    .sort((a, b) => b.updatedAtMs - a.updatedAtMs)[0];
  const done = mockOutbox
    .filter((row) => row.eventType.startsWith(`${provider}.`) && row.state === "done")
    .sort((a, b) => b.updatedAtMs - a.updatedAtMs)[0];
  return { lastError: failed?.lastError ?? null, lastSyncAt: done?.updatedAtMs ?? null };
}

function mockIntegrationView(id: string): IntegrationView {
  const meta = INTEGRATION_META.find((item) => item.id === id);
  if (!meta) throw makeError("E_UNKNOWN", `未知集成：${id}`);
  const config = mockPluginConfig[id] ?? {};
  const required = id === "memos" ? "instanceUrl" : id === "ai" ? "baseUrl" : "";
  const configured =
    required === ""
      ? true
      : typeof config[required] === "string" && String(config[required]).trim() !== "";
  return {
    id: meta.id,
    name: meta.name,
    version: meta.version,
    installed: config.installed !== false,
    enabled: mockEnabled[id] ?? false,
    configured,
    capabilities: meta.capabilities,
    ...mockProviderStatus(id)
  };
}

function mockIntegrationSetEnabled(args?: Record<string, unknown>): IntegrationView {
  const id = String(args?.id ?? "");
  if (!INTEGRATION_META.some((item) => item.id === id)) {
    throw makeError("E_UNKNOWN", `未知集成：${id}`);
  }
  mockEnabled[id] = Boolean(args?.enabled);
  return mockIntegrationView(id);
}

function mockIntegrationConfigSet(args?: Record<string, unknown>): IntegrationView {
  const id = String(args?.id ?? "");
  if (!INTEGRATION_META.some((item) => item.id === id)) {
    throw makeError("E_UNKNOWN", `未知集成：${id}`);
  }
  const config = (args?.config ?? {}) as Record<string, unknown>;
  for (const key of ["instanceUrl", "baseUrl"]) {
    const url = config[key];
    if (typeof url === "string" && url.trim() !== "" && !/^https?:\/\//.test(url.trim())) {
      throw makeError("E_SETTING_INVALID", `${key} 需要以 http(s):// 开头`);
    }
  }
  mockPluginConfig[id] = { ...(mockPluginConfig[id] ?? {}), ...config };
  return mockIntegrationView(id);
}

function mockIntegrationTest(id: string): IntegrationTestResult {
  const view = mockIntegrationView(id);
  if (!view.configured) {
    throw makeError("E_INTEGRATION_NOT_CONFIGURED", "请先填写服务地址");
  }
  if (!view.enabled) {
    throw makeError("E_PLUGIN_DISABLED", "该集成未启用，请先在集成页打开");
  }
  return {
    ok: true,
    message: id === "memos" ? "已连接 · v0.24 (stable API)" : "连接成功"
  };
}

function mockAiSuggest(date?: string): AiSuggestion[] {
  if (!(mockEnabled.ai ?? false)) {
    throw makeError("E_PLUGIN_DISABLED", "AI 助手未启用");
  }
  const range=dayWindowOf(new Date(`${date||dayKeyOf(Date.now())}T12:00:00`));
  const candidates = mockAllSegments()
    .filter((segment) => !segment.isIgnored && segment.status === "closed" && segment.startAt<range.to && (segment.endAt??0)>range.from)
    .sort((left, right) => right.startAt - left.startAt)
    .slice(0, 4);
  if (candidates.length === 0) return [];
  return candidates.map((segment, index) => {
    const category = MOCK_CATEGORIES[index % MOCK_CATEGORIES.length];
    const base = segment.title || "未命名记录";
    return {
      segmentId: segment.id,
      revision: segment.revision,
      title: base.length <= 12 ? base : base.slice(0, 12),
      tags: index % 2 === 0 ? ["专注", "复盘"] : ["碎片"],
      categoryId: segment.category === null ? category.id : segment.category.id,
      reason: `与「${category.name}」的既有记录更接近`
    };
  });
}

function mockAiApply(args?: Record<string, unknown>): SegmentView {
  const input = (args?.input ?? {}) as AiApplyInput;
  const target = mockAllSegments().find((segment) => segment.id === input.segmentId);
  if (!target) throw makeError("E_SEGMENT_NOT_FOUND", "这条记录不存在");
  if (target.revision !== input.revision) {
    throw makeError("E_STALE_REVISION", "记录已被修改，请重新获取建议");
  }

  const newTags: TagView[] = (input.tags ?? [])
    .map((tag) => tag.trim().replace(/^#/, ""))
    .filter((tag) => tag !== "" && !target.tags.some((existing) => existing.name === tag))
    .map((tag) => ({ id: nextId("tag"), name: tag, color: null }));

  const updated: SegmentView = {
    ...target,
    title: input.title?.trim() ? input.title.trim() : target.title,
    category:
      input.categoryId == null || input.categoryId.trim() === ""
        ? target.category
        : MOCK_CATEGORIES.find((category) => category.id === input.categoryId) ?? target.category,
    tags: [...target.tags, ...newTags],
    revision: target.revision + 1
  };
  if (mockActive?.id === updated.id) {
    mockActive = updated;
  } else {
    mockClosed = mockClosed.map((segment) => (segment.id === updated.id ? updated : segment));
  }
  pushPoint("edit", Date.now(), updated.id, { restore: [target], removeIds: [] });
  return updated;
}

async function mockCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  switch (command) {
    case "timer_get_state":
      return mockState() as T;
    case "timer_start": {
      if (mockActive) throw makeError("E_ACTIVE_EXISTS", "已存在进行中的记录");
      const at = Date.now(); const resuming = mockPaused;
      if (!resuming) mockSessionIds = [];
      mockActive = mockSegment(at); mockSessionIds.push(mockActive.id); mockSessionGroups[mockActive.id] = mockSessionIds;
      mockPaused = false; pushPoint(resuming ? "resume" : "start", at, mockActive.id);
      return mockState() as T;
    }
    case "timer_punch": {
      const closed = mockClose(); mockPaused = false;
      pushPoint("punch", closed.endAt ?? Date.now(), closed.id);
      const result = mockResult(closed); mockSessionIds = [];
      if (mockSettings["punch.mode"] === "chain") { mockActive = mockSegment(closed.endAt ?? Date.now()); mockSessionIds.push(mockActive.id); mockSessionGroups[mockActive.id] = mockSessionIds; }
      return { ...result, newSegment: mockActive } as T;
    }
    case "timer_pause": {
      const closed = mockClose(); mockPaused = true;
      pushPoint("pause", closed.endAt ?? Date.now(), closed.id);
      return { ...mockResult(closed), pendingNote: null } as T;
    }
    case "timer_finish": {
      const wasPaused = mockPaused;
      const closed = wasPaused ? mockClosed.find((s) => s.id === mockSessionIds[mockSessionIds.length - 1]) : mockClose();
      if (!closed) throw makeError("E_NOT_RECORDING", "没有可结束的记录");
      mockPaused = false; pushPoint("finish", Date.now(), closed.id);
      mockPoints[mockPoints.length - 1].pausedFinish = wasPaused;
      const result = mockResult(closed); mockSessionIds = []; return result as T;
    }
    case "timer_undo":
      return mockUndo() as T;
    case "day_today": return mockDay() as T;
    case "day_get": return mockDay(String(args?.date ?? "")) as T;
    case "segment_update":
      return mockUpdateSegment(args) as T;
    case "segment_backfill":
      return mockBackfill((args?.input ?? {}) as BackfillInput) as T;
    case "segment_delete":
      mockDelete(String(args?.id ?? ""));
      return undefined as T;
    case "segment_split":
      return mockSplit(String(args?.id ?? ""), Number(args?.at ?? 0)) as T;
    case "segment_merge":
      return mockMerge((args?.ids as string[]) ?? []) as T;
    case "history_query":
      return mockHistory(args) as T;
    case "stats_get":
      return mockStats(args) as T;
    case "categories_list":
      return MOCK_CATEGORIES as T;
    case "settings_get":
      return mockSettingsGet(args) as T;
    case "settings_patch":
      return mockSettingsPatch(args) as T;
    case "settings_reset":
      mockSettingsReset(args);
      return undefined as T;
    case "export_data":
      return mockExport(args) as T;
    case "import_preview":
      return analyzeMockImport(String(args?.content ?? "")) as T;
    case "import_commit":
      return mockImportCommit(args) as T;
    case "backup_now":
      return mockBackupNow() as T;
    case "backup_list":
      return mockBackups as T;
    case "backup_restore":
      mockBackupRestore(String(args?.path ?? ""));
      return undefined as T;
    case "recovery_get":
      return mockRecoveryGet() as T;
    case "recovery_resolve":
      return mockResolveRecovery((args?.action ?? "extend") as RecoveryAction) as T;
    case "diagnostics_snapshot":
      return mockDiagnostics() as T;
    case "pending_note_get": return null as T;
    case "segment_confirm": return undefined as T;
    case "segment_get": {
      const segment = [mockActive, ...mockClosed].find((s) => s?.id === args?.id);
      if (!segment) throw makeError("E_SEGMENT_NOT_FOUND", "记录不存在");
      return segment as T;
    }
    case "memos_browse":
      if (!mockEnabled.memos || mockPluginConfig.memos?.installed === false) throw makeError("E_PLUGIN_DISABLED", "请先添加并启用 Memos 插件");
      return { user: "预览用户", userId: "users/preview", instanceUrl: mockPluginConfig.memos?.instanceUrl ?? "", memos: mockMemos, nextPageToken: null, apiVersion: "v1" } as T;
    case "memos_save": {
      const name = String(args?.name ?? `memos/preview-${Date.now()}`);
      const memo: Memo = { name, content: String(args?.content ?? ""), visibility: String(args?.visibility ?? "PRIVATE"), createTime: new Date().toISOString() };
      mockMemos = [memo, ...mockMemos.filter((m) => m.name !== name)];
      return memo as T;
    }
    case "memos_delete":
      mockMemos = mockMemos.filter((m) => m.name !== args?.name); return undefined as T;
    case "integration_list":
      return INTEGRATION_META.map((meta) => mockIntegrationView(meta.id)) as T;
    case "integration_set_installed": {
      const id = String(args?.id ?? "");
      mockPluginConfig[id] = { ...(mockPluginConfig[id] ?? {}), installed: Boolean(args?.installed) };
      return mockIntegrationSetEnabled({ id, enabled: false }) as T;
    }
    case "integration_set_enabled":
      return mockIntegrationSetEnabled(args) as T;
    case "integration_config_set":
      return mockIntegrationConfigSet(args) as T;
    case "integration_secret_set": {
      const id = String(args?.id ?? "");
      const secret = String(args?.secret ?? "");
      if (secret.trim() === "") delete mockSecrets[`${id}:token`];
      else mockSecrets[`${id}:token`] = secret.trim();
      return undefined as T;
    }
    case "integration_test":
      return mockIntegrationTest(String(args?.id ?? "")) as T;
    case "outbox_list": {
      const state = args?.state;
      const rows =
        typeof state === "string" ? mockOutbox.filter((row) => row.state === state) : mockOutbox;
      return [...rows].sort((left, right) => right.updatedAtMs - left.updatedAtMs) as T;
    }
    case "outbox_retry": {
      const id = String(args?.id ?? "");
      const row = mockOutbox.find((item) => item.id === id);
      if (!row) throw makeError("E_UNKNOWN", "事件不存在");
      row.state = "done";
      row.lastError = null;
      row.nextRetryAtMs = null;
      row.attempts += 1;
      row.updatedAtMs = Date.now();
      return { ...row } as T;
    }
    case "ai_suggest":
      return mockAiSuggest(args?.date ? String(args.date) : undefined) as T;
    case "ai_models": return ["演示模型", "qwen2.5:7b"] as T;
    case "ai_review": {
      const input=args?.input as {from:string;to:string;question:string};
      const day=mockDay(input.to);
      return {from:input.from,to:input.to,recordCount:day.segments.length,recordedSeconds:day.stats.recordedSeconds,model:"演示模型",
        markdown:"## 本期概览\n这是浏览器模拟复盘，用于检查界面，不是真实模型分析。\n\n## 时间分配\n所选记录包含英语精读和工作活动。\n\n## 下一步建议\n- 给学习段补充具体章节。\n- 结合当天记录安排下一次复盘。"} as T;
    }
    case "ai_apply":
      return mockAiApply(args) as T;
    case "tomatodo_scan":
      return mockTomatodoScan() as T;
    case "tomatodo_import":
      return mockTomatodoImport() as T;
    case "tomatodo_compare":
      return mockTomatodoCompare() as T;
    case "tomatodo_analyze":
      return mockTomatodoAnalyze() as T;
    default:
      throw makeError("E_UNKNOWN", `未知命令：${command}`);
  }
}

// ---------- 番茄Todo mock ----------

function mockTomatodoScan(): TomatodoScanResult {
  const now = Date.now();
  return {
    dbPath: "%APPDATA%\\tomatodo\\tomatodo_db.json",
    fileMtimeMs: now - 5 * 60_000,
    totalRecords: 4,
    completedRecords: 3,
    totalMinutes: 121,
    earliestStartMs: now - 7 * 24 * 3_600_000,
    latestEndMs: now - 2 * 24 * 3_600_000,
    todoCount: 6,
    preview: [
      {
        id: 4,
        name: "高数",
        startAtMs: now - 2 * 24 * 3_600_000,
        endAtMs: now - 2 * 24 * 3_600_000 + 105 * 60_000,
        durationMinutes: 105,
        startText: "10-27 02:02",
        endText: "10-27 03:47"
      },
      {
        id: 2,
        name: "线代",
        startAtMs: now - 3 * 24 * 3_600_000,
        endAtMs: now - 3 * 24 * 3_600_000 + 60_000,
        durationMinutes: 1,
        startText: "10-26 21:49",
        endText: "10-26 21:50"
      },
      {
        id: 1,
        name: "高数",
        startAtMs: now - 7 * 24 * 3_600_000,
        endAtMs: now - 7 * 24 * 3_600_000 + 15 * 60_000,
        durationMinutes: 15,
        startText: "10-22 09:20",
        endText: "10-22 09:35"
      }
    ]
  };
}

function mockTomatodoImport(): TomatodoImportResult {
  return {
    imported: 3,
    skippedExisting: 0,
    skippedIncomplete: 1,
    conflicts: 0,
    errors: []
  };
}

function mockTomatodoCompare(): TomatodoCompareResult {
  return {
    summary: {
      tomatodoMinutes: 121,
      dotlogMinutes: 90,
      missingInDotlog: 1,
      extraInDotlog: 2,
      matchedPairs: 2
    },
    pairs: [
      {
        tomatodo: {
          id: 4,
          name: "高数",
          startAtMs: Date.now() - 2 * 24 * 3_600_000,
          endAtMs: Date.now() - 2 * 24 * 3_600_000 + 105 * 60_000,
          durationMinutes: 105,
          startText: "10-27 02:02",
          endText: "10-27 03:47"
        },
        dotlogSegments: [
          {
            id: "seg-1",
            startAtMs: Date.now() - 2 * 24 * 3_600_000,
            endAtMs: Date.now() - 2 * 24 * 3_600_000 + 60 * 60_000,
            title: "做高数 880 题 21-35",
            categoryName: "学习",
            tags: ["高数", "刷题"]
          },
          {
            id: "seg-2",
            startAtMs: Date.now() - 2 * 24 * 3_600_000 + 60 * 60_000,
            endAtMs: Date.now() - 2 * 24 * 3_600_000 + 105 * 60_000,
            title: "整理高数错题",
            categoryName: "学习",
            tags: ["高数", "错题"]
          }
        ],
        overlapSeconds: 105 * 60,
        tomatodoSeconds: 105 * 60,
        dotlogSeconds: 105 * 60,
        coveragePercent: 100
      },
      {
        tomatodo: {
          id: 2,
          name: "线代",
          startAtMs: Date.now() - 3 * 24 * 3_600_000,
          endAtMs: Date.now() - 3 * 24 * 3_600_000 + 60_000,
          durationMinutes: 1,
          startText: "10-26 21:49",
          endText: "10-26 21:50"
        },
        dotlogSegments: [],
        overlapSeconds: 0,
        tomatodoSeconds: 60,
        dotlogSeconds: 0,
        coveragePercent: 0
      },
      {
        tomatodo: null,
        dotlogSegments: [
          {
            id: "seg-3",
            startAtMs: Date.now() - 4 * 24 * 3_600_000,
            endAtMs: Date.now() - 4 * 24 * 3_600_000 + 30 * 60_000,
            title: "自由阅读",
            categoryName: "阅读",
            tags: []
          }
        ],
        overlapSeconds: 0,
        tomatodoSeconds: 0,
        dotlogSeconds: 30 * 60,
        coveragePercent: 0
      }
    ]
  };
}

function mockTomatodoAnalyze(): TomatodoAnalysis[] {
  // 只对有 DotLog 段的配对做分析（与 tomatodo_analyze 的过滤逻辑一致）。
  return [
    {
      pairIndex: 0,
      tomatodoSubject: "高数",
      dotlogSummary: "做高数 880 题 21-35（学习）；整理高数错题（学习）",
      suggestion: "匹配良好：DotLog 记录了高数刷题和错题整理，与番茄Todo 科目一致。",
      reason: "番茄Todo 科目「高数」与 DotLog 活动高度匹配，记录粒度合理。"
    }
  ];
}
