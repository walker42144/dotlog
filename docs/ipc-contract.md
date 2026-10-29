# 打点 DotLog · IPC 契约（v1）

版本：IPC_VERSION = 1 ｜ 生成方式：Rust `tauri-specta` → `packages/ipc/src/bindings.ts`

本文是前后端唯一契约。所有类型先定义在 Rust（`application::dto`），再生成 TS；前端不得手写重复类型。

---

## 1. 基本约定

| 约定 | 规则 |
|---|---|
| 时间戳 | epoch 毫秒（number）；UTC 语义 |
| 日期 | 本地日期字符串 `YYYY-MM-DD`（按 `day_start_minutes` 归属） |
| 时长 | 秒（整数，向下取整）；展示格式由 UI 负责 |
| ID | UUIDv7 字符串 |
| 布尔 | `boolean`，DB 内部 0/1 |
| 空值 | `null`，不使用 `undefined` |
| 错误 | 统一抛出 `IpcError { code, message, detail? }` |
| 事件 | 单向推送，命名 `域://动作`，payload 为对象 |

---

## 2. 类型定义（TS）

```ts
export type Phase = 'idle' | 'recording';
export type PunchMode = 'stop' | 'chain';
export type NoticeKind = 'undo' | 'long_gap' | 'clock' | 'sync' | 'info';

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
  startAt: number;              // epoch ms
  endAt: number | null;         // active 段为 null
  durationSeconds: number;      // active 段 = now - startAt
  title: string;
  note: string | null;
  category: CategoryView | null;
  tags: TagView[];
  isFocus: boolean;
  isIgnored: boolean;
  status: 'active' | 'closed';
  source: 'manual' | 'hotkey' | 'tray' | 'system' | 'plugin' | 'import';
  revision: number;
}

export interface GapView {
  startAt: number;
  endAt: number;
  seconds: number;
}

export interface DayStats {
  recordedSeconds: number;
  effectiveSeconds: number;
  blankSeconds: number;
  focusSeconds: number;
  coverageRate: number;         // 0–1
  switchCount: number;
  segmentCount: number;
  averageSegmentSeconds: number;
}

export interface DayView {
  date: string;                 // 'YYYY-MM-DD'
  windowStart: number;          // epoch ms
  windowEnd: number;
  segments: SegmentView[];      // 已按日期切片；带原始 segmentId
  gaps: GapView[];
  stats: DayStats;
}

export interface PendingNote {
  segmentId: string;
  startAt: number;
  endAt: number;
}

export interface TimerState {
  phase: Phase;
  active: SegmentView | null;
  elapsedSeconds: number;
  mode: PunchMode;
  today: DayStats;
  pendingNote: PendingNote | null;
  notices: Notice[];
}

export interface Notice {
  id: string;
  kind: NoticeKind;
  message: string;
  actionable: boolean;
  createdAt: number;
}

export interface PunchResult {
  closedSegment: SegmentView;
  newSegment: SegmentView | null;   // stop 模式为 null
  pendingNote: PendingNote | null;
  undoToken: string | null;         // 15 秒内有效
  shortSegment: boolean;            // 关闭段 < 15 秒
}

export interface UndoResult {
  restoredSegment: SegmentView | null;
  removedSegmentId: string | null;
}

export interface RecoveryInfo {
  required: boolean;
  reason: 'cross_day' | 'long_gap' | 'clock_anomaly' | 'orphan_active' | null;
  activeSegment: SegmentView | null;
  detectedAt: number;
  options: Array<'truncate' | 'extend' | 'discard' | 'keep'>;
}
```

---

## 3. 命令

### 3.1 计时

| 命令 | 入参 | 返回 | 可能错误 |
|---|---|---|---|
| `timer_get_state` | — | `TimerState` | `E_DB` |
| `timer_start` | `{ source?: PointSource }` | `TimerState` | `E_ACTIVE_EXISTS`、`E_CLOCK_ANOMALY` |
| `timer_punch` | `{ source?: PointSource }` | `PunchResult` | `E_NOT_RECORDING`、`E_INVALID_RANGE` |
| `timer_pause` | `{ source?: PointSource }` | `PunchResult` | `E_NOT_RECORDING` |
| `timer_finish` | `{ source?: PointSource }` | `PunchResult` | `E_NOT_RECORDING` |
| `timer_undo` | `{ token?: string }` | `UndoResult` | `E_UNDO_EXPIRED`、`E_UNDO_UNAVAILABLE` |

说明：

- `source` 缺省为 `button`。
- `timer_punch` 的 `pendingNote` 在 `stop` 模式下始终非空（刚关闭的段）；`chain` 模式同样返回被关闭段。
- 前端不得在调用前修改本地状态；以返回值为准。
- 显示用秒数：前端用 `startAt` + 本地时钟做 1 s 刷新；每次命令返回后重新对齐。

### 3.2 时间段

| 命令 | 入参 | 返回 | 说明 |
|---|---|---|---|
| `segment_get` | `{ id }` | `SegmentView` | 详情 |
| `segment_update` | `{ id, revision, patch }` | `SegmentView` | 乐观锁，`revision` 不匹配返回 `E_STALE_REVISION` |
| `segment_backfill` | `{ startAt, endAt, title?, note?, tagIds?, categoryId?, isFocus? }` | `SegmentView` | 创建已关闭段；冲突拒绝 |
| `segment_delete` | `{ id }` | `void` | 软删除；active 段先自动关闭 |
| `segment_split` | `{ id, at }` | `SegmentView[]` | 在指定时刻拆成两段（M2） |
| `segment_merge` | `{ ids }` | `SegmentView` | 合并连续段，中间空档一并覆盖（M2） |

`SegmentPatch` 字段：`title?`、`note?`、`startAt?`、`endAt?`、`categoryId?`、`tagIds?`、`isFocus?`、`isIgnored?`。`undefined` 表示不修改。

编辑规则：

- `startAt` 必须 `< now` 且 `< endAt`，且不与前段重叠。
- `endAt` 必须 `> startAt`，且不晚于下一段起点。
- active 段不可传 `endAt`（用结束命令）。

### 3.3 日视图与查询

| 命令 | 入参 | 返回 |
|---|---|---|
| `day_get` | `{ date }` | `DayView` |
| `day_today` | — | `DayView` |
| `history_query` | `{ from, to, text?, categoryIds?, tagIds?, onlyFocus?, limit, offset }` | `{ items: SegmentView[], total }` |
| `stats_get` | `{ from, to, granularity: 'day' \| 'week' \| 'month' }` | `{ buckets: StatsBucket[] }` |

`StatsBucket { dateStart, dateEnd, recordedSeconds, effectiveSeconds, focusSeconds, blankSeconds, coverageRate, byCategory: { categoryId, seconds }[] }`

### 3.4 设置

| 命令 | 入参 | 返回 |
|---|---|---|
| `settings_get` | `{ scope?: string }` | `Record<string, unknown>` |
| `settings_patch` | `{ scope: string, entries: { key, value }[] }` | `Record<string, unknown>` |
| `settings_reset` | `{ scope: string, key?: string }` | `void` |

设置项键名（应用 scope = `app`）：`punch.mode`、`punch.undo_window_ms`、`punch.short_segment_ms`、`day.start_minutes`、`day.active_window`、`focus.min_minutes`、`hotkey.punch`、`notify.on_punch`、`launch.at_login`、`ui.theme`、`backup.enabled`、`backup.keep_daily`、`backup.keep_weekly`、`backup.keep_monthly`。

### 3.5 恢复、备份与集成

| 命令 | 入参 | 返回 |
|---|---|---|
| `recovery_get` | — | `RecoveryInfo` |
| `recovery_resolve` | `{ action: 'truncate' \| 'extend' \| 'discard' \| 'keep' }` | `TimerState` |
| `backup_now` | — | `{ path, createdAt, sizeBytes }` |
| `backup_list` | — | `BackupEntry[]` |
| `backup_restore` | `{ path }` | `void`（成功后应用重启） |
| `export_data` | `{ format: 'json' \| 'csv', from, to, path? }` | `{ path, count }` |
| `integration_list` | — | `IntegrationView[]` |
| `integration_set_enabled` | `{ id, enabled }` | `IntegrationView` |
| `integration_config_set` | `{ id, config: object }` | `IntegrationView` |
| `integration_secret_set` | `{ id, secret }` | `void`（写入系统钥匙串） |
| `integration_test` | `{ id }` | `{ ok, message }` |
| `outbox_list` | `{ state? }` | `OutboxView[]` |
| `outbox_retry` | `{ id }` | `OutboxView` |
| `diagnostics_snapshot` | — | `{ path }` |

`IntegrationView { id, name, enabled, configured, capabilities, lastError, lastSyncAt }`

---

## 4. 事件（后端 → 前端）

| 事件名 | payload | 触发时机 |
|---|---|---|
| `timer://changed` | `TimerState` | 打点、暂停、恢复、撤销、恢复处理、跨日 |
| `day://invalidated` | `{ date: string }` | 日期切片变化、编辑生效 |
| `app://notice` | `Notice` | 短段提示、同步失败、备份失败 |
| `recovery://required` | `RecoveryInfo` | 启动时检测到异常 |
| `integration://status` | `IntegrationView` | 同步状态变化 |

托盘与主窗口都订阅 `timer://changed`，托盘标题由 Rust 直接更新，不依赖前端。

---

## 5. 错误码

| code | 含义 | 前端处理 |
|---|---|---|
| `E_NOT_RECORDING` | 当前没有 active 段 | 提示“当前没有进行中的记录”，刷新状态 |
| `E_ACTIVE_EXISTS` | 已存在 active 段 | 引导恢复/结束 |
| `E_INVALID_RANGE` | 起止时间非法 | 高亮输入框 |
| `E_OVERLAP` | 与已有段重叠 | 展示冲突段，提供裁剪/拆分 |
| `E_SEGMENT_NOT_FOUND` | 段不存在或已删 | 刷新日视图 |
| `E_STALE_REVISION` | 乐观锁冲突 | 重新拉取后提示 |
| `E_UNDO_EXPIRED` | 撤销窗口已过 | 提示改用手动编辑 |
| `E_UNDO_UNAVAILABLE` | 无可用事实 | 同 `E_UNDO_EXPIRED` |
| `E_CLOCK_ANOMALY` | 系统时间异常 | 阻塞并引导恢复流程 |
| `E_DB_BUSY` | 数据库忙 | 自动重试一次，再提示 |
| `E_DB_CORRUPT` | 完整性检查失败 | 引导从备份恢复 |
| `E_SETTING_INVALID` | 设置值非法 | 字段级报错 |
| `E_BACKUP_FAILED` | 备份失败 | 通知 + 诊断入口 |
| `E_RESTORE_INVALID` | 备份不可用 | 保持当前库 |
| `E_INTEGRATION_NOT_CONFIGURED` | 集成未配置 | 跳转集成页 |
| `E_NETWORK` | 网络失败 | 入 outbox 重试 |
| `E_PLUGIN_DISABLED` / `E_PLUGIN_TIMEOUT` | 插件不可用/超时 | 记录日志，不阻塞打点 |
| `E_UNKNOWN` | 兜底 | 显示诊断 ID |

`detail` 结构示例：

```json
{ "code": "E_OVERLAP", "message": "与已有记录重叠", "detail": { "conflicts": [{ "id": "...", "startAt": 0, "endAt": 0, "title": "英语" }] } }
```

---

## 6. 生成与兼容

- `cargo test --features bindings export_bindings` 生成 `packages/ipc/src/bindings.ts`；CI 校验生成文件无 diff。
- 兼容规则：仅允许**加字段/加命令/加事件**；删除或改语义必须提升 `IPC_VERSION` 并在 `ui` 内做适配。
- 前端统一通过 `packages/ipc/client.ts` 调用，禁止页面直接 `invoke`，便于集中处理错误与埋点（本地日志）。

---

## 7. 前后端职责边界

| 事项 | Rust | 前端 |
|---|---|---|
| 起止时间、时长 | ✅ 权威 | 仅显示 |
| 统计计算 | ✅ 唯一实现 | 不计算，只渲染 |
| 日切片/空白 | ✅ | 渲染 |
| 冲突校验 | ✅ | 提供裁剪选项 |
| 表单校验（长度、空值） | 二次校验 | 首次校验 |
| 主题、键盘、动画 | — | ✅ |
| 托盘标题 | ✅ | — |
