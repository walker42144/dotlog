# 打点 DotLog · 数据库设计与数据字典

版本：v0.1 ｜ 引擎：SQLite（`rusqlite` bundled） ｜ schema_version：1

文件位置：`%APPDATA%\dotlog\dotlog.db`；备份：`%APPDATA%\dotlog\backups\`。

---

## 1. 设计原则

1. **事实与展示分离**：`time_point` 只记录打点事实；`time_segment` 是用户看到与编辑的实体。
2. **软删除**：业务删除一律 `status='deleted'` + `deleted_at_ms`，便于撤销与未来同步。
3. **UTC 毫秒 + 时区偏移**：避免夏令时与跨时区歧义。
4. **数据库兜底关键约束**：单 active 段由唯一索引保证，而不只靠代码。
5. **迁移可回滚**：迁移前自动备份，失败保留旧库。
6. **插件隔离**：插件无数据库句柄，仅通过应用服务与 outbox 间接写入。

---

## 2. 初始化 PRAGMA

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;
```

---

## 3. 迁移管理

```sql
CREATE TABLE IF NOT EXISTS migration (
  version       INTEGER PRIMARY KEY,
  name          TEXT    NOT NULL,
  checksum      TEXT    NOT NULL,
  applied_at_ms INTEGER NOT NULL
) STRICT;
```

- 迁移文件：`crates/storage/migrations/0001_init.sql`，编译期 `include_str!` 嵌入。
- 执行顺序：备份 → `BEGIN IMMEDIATE` → 逐条执行 → 写入 `migration` → `PRAGMA user_version = version` → `COMMIT`。
- 校验：文件 sha256 与已应用 checksum 不一致时拒绝启动并提示（防止手改历史迁移）。
- 回滚：SQLite DDL 支持事务；失败整体回滚。破坏性迁移必须先复制库文件。

---

## 4. 完整 DDL（迁移 0001）

```sql
-- 分类：轻量层级，根节点 parent_id = ''
CREATE TABLE category (
  id             TEXT PRIMARY KEY,
  name           TEXT NOT NULL,
  color          TEXT NOT NULL DEFAULT '#8A8F98',
  parent_id      TEXT NOT NULL DEFAULT '',
  sort_order     INTEGER NOT NULL DEFAULT 0,
  archived_at_ms INTEGER,
  created_at_ms  INTEGER NOT NULL,
  updated_at_ms  INTEGER NOT NULL,
  UNIQUE (parent_id, name)
) STRICT;

-- 标签：多对多、大小写不敏感
CREATE TABLE tag (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL COLLATE NOCASE,
  color           TEXT,
  last_used_at_ms INTEGER,
  created_at_ms   INTEGER NOT NULL,
  updated_at_ms   INTEGER NOT NULL,
  UNIQUE (name)
) STRICT;

-- 时间段：用户可见与编辑的实体
CREATE TABLE time_segment (
  id                  TEXT PRIMARY KEY,
  start_at_ms         INTEGER NOT NULL,
  end_at_ms           INTEGER,
  start_tz_offset_min INTEGER NOT NULL,
  end_tz_offset_min   INTEGER,
  title               TEXT    NOT NULL DEFAULT '',
  note                TEXT,
  status              TEXT    NOT NULL DEFAULT 'active'
                      CHECK (status IN ('active','closed','deleted')),
  category_id         TEXT REFERENCES category(id) ON DELETE SET NULL,
  is_focus            INTEGER NOT NULL DEFAULT 0 CHECK (is_focus IN (0,1)),
  is_ignored          INTEGER NOT NULL DEFAULT 0 CHECK (is_ignored IN (0,1)),
  source              TEXT    NOT NULL DEFAULT 'manual'
                      CHECK (source IN ('manual','hotkey','tray','system','plugin','import')),
  revision            INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  created_at_ms       INTEGER NOT NULL,
  updated_at_ms       INTEGER NOT NULL,
  deleted_at_ms       INTEGER,
  CHECK (
    (status = 'active'  AND end_at_ms IS NULL)
    OR
    (status <> 'active' AND end_at_ms IS NOT NULL AND end_at_ms > start_at_ms)
  ),
  CHECK ((status = 'deleted') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX ux_segment_single_active
  ON time_segment(status) WHERE status = 'active';
CREATE INDEX ix_segment_range  ON time_segment(start_at_ms, end_at_ms);
CREATE INDEX ix_segment_status ON time_segment(status, start_at_ms);
CREATE INDEX ix_segment_title  ON time_segment(title);
CREATE INDEX ix_segment_cat    ON time_segment(category_id);

-- 打点事实：只追加，撤销只标记
CREATE TABLE time_point (
  id              TEXT PRIMARY KEY,
  occurred_at_ms  INTEGER NOT NULL,
  tz_offset_min   INTEGER NOT NULL,
  kind            TEXT NOT NULL
                  CHECK (kind IN ('start','punch','pause','resume','finish','undo','edit','backfill','delete')),
  source          TEXT NOT NULL
                  CHECK (source IN ('hotkey','button','tray','plugin','import','system')),
  segment_id      TEXT REFERENCES time_segment(id) ON DELETE SET NULL,
  payload         TEXT,
  undone_at_ms    INTEGER,
  created_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_point_occurred ON time_point(occurred_at_ms);
CREATE INDEX ix_point_segment  ON time_point(segment_id);

-- 段 ↔ 标签
CREATE TABLE segment_tag (
  segment_id    TEXT NOT NULL REFERENCES time_segment(id) ON DELETE CASCADE,
  tag_id        TEXT NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
  created_at_ms INTEGER NOT NULL,
  PRIMARY KEY (segment_id, tag_id)
) STRICT;

-- 外部对象回链（Memos 等）
CREATE TABLE integration_link (
  segment_id        TEXT NOT NULL REFERENCES time_segment(id) ON DELETE CASCADE,
  provider          TEXT NOT NULL,
  external_id       TEXT,
  state             TEXT NOT NULL
                    CHECK (state IN ('pending','published','failed','removed')),
  last_synced_at_ms INTEGER,
  last_error        TEXT,
  PRIMARY KEY (segment_id, provider),
  UNIQUE (provider, external_id)
) STRICT;

-- 可靠异步发送
CREATE TABLE outbox_event (
  id              TEXT PRIMARY KEY,
  dedupe_key      TEXT UNIQUE,
  type            TEXT NOT NULL,
  payload         TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'pending'
                  CHECK (state IN ('pending','inflight','done','failed')),
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_retry_at_ms INTEGER,
  last_error      TEXT,
  created_at_ms   INTEGER NOT NULL,
  updated_at_ms   INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_outbox_due ON outbox_event(state, next_retry_at_ms);

-- 命名空间配置（应用 / 插件）
CREATE TABLE setting (
  scope         TEXT NOT NULL,
  key           TEXT NOT NULL,
  value         TEXT NOT NULL,          -- JSON
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (scope, key)
) STRICT;

-- 运行时状态（心跳、上次活跃、时钟基准）
CREATE TABLE runtime_state (
  key           TEXT PRIMARY KEY,
  value         TEXT NOT NULL,
  updated_at_ms INTEGER NOT NULL
) STRICT;
```

说明：

- `STRICT` 表要求字段类型严格；SQLite 3.37+ 支持，bundled 版本满足。
- `category.parent_id` 用空串而非 `NULL`，使 `UNIQUE(parent_id, name)` 真正生效（SQLite 中 `NULL` 互不相等）。
- `integration_link` 的 `UNIQUE(provider, external_id)` 允许多个 `NULL`，正好用于“尚未发布”状态。
- 段重叠是跨行约束，SQL 约束无法表达，由 application 写前校验 + 启动自检兜底。

---

## 5. 字段字典

### 5.1 `time_segment`

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `id` | TEXT | 是 | UUIDv7，可排序 |
| `start_at_ms` | INTEGER | 是 | 起始 UTC 毫秒 |
| `end_at_ms` | INTEGER | active 时为空 | 结束 UTC 毫秒 |
| `start_tz_offset_min` | INTEGER | 是 | 起始时本地时区偏移（分钟，东为正） |
| `end_tz_offset_min` | INTEGER | 关闭时必填 | 结束时偏移 |
| `title` | TEXT | 是（可为空串） | 一句话标题，≤ 200 字符 |
| `note` | TEXT | 否 | 备注，≤ 5 000 字符 |
| `status` | TEXT | 是 | `active` / `closed` / `deleted` |
| `category_id` | TEXT | 否 | 关联分类 |
| `is_focus` | INTEGER | 是 | 0/1，专注标记 |
| `is_ignored` | INTEGER | 是 | 0/1，统计忽略 |
| `source` | TEXT | 是 | 创建来源 |
| `revision` | INTEGER | 是 | 每次修改 +1 |
| `created_at_ms` | INTEGER | 是 | 创建时间 |
| `updated_at_ms` | INTEGER | 是 | 更新时间 |
| `deleted_at_ms` | INTEGER | 删除时必填 | 软删除时间 |

### 5.2 `time_point`

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | TEXT | UUIDv7 |
| `occurred_at_ms` | INTEGER | 事实发生时间（UTC ms） |
| `tz_offset_min` | INTEGER | 发生时区偏移 |
| `kind` | TEXT | `start` / `punch` / `pause` / `resume` / `finish` / `undo` / `edit` / `backfill` / `delete` |
| `source` | TEXT | `hotkey` / `button` / `tray` / `plugin` / `import` / `system` |
| `segment_id` | TEXT | 主要关联段（punch 记录被关闭段） |
| `payload` | TEXT (JSON) | 撤销所需旧值、恢复原因等 |
| `undone_at_ms` | INTEGER | 被撤销时间；NULL 表示有效 |
| `created_at_ms` | INTEGER | 写入时间 |

### 5.3 `outbox_event`

| 字段 | 说明 |
|---|---|
| `dedupe_key` | 去重键，如 `memos:<segment_id>:publish`，防止同一段重复入队 |
| `type` | 如 `memos.publish` / `ai.classify` |
| `payload` | JSON，事件快照（不引用后续可变状态） |
| `state` | `pending` → `inflight` → `done` / `failed` |
| `attempts` | 已尝试次数，最大 6 |
| `next_retry_at_ms` | 下次重试时间 |

---

## 6. 常用查询

今日段（含 active）：

```sql
SELECT * FROM time_segment
WHERE status IN ('active','closed')
  AND start_at_ms < :day_end
  AND (end_at_ms IS NULL OR end_at_ms > :day_start)
ORDER BY start_at_ms;
```

最近标签（补记框提供）：

```sql
SELECT t.id, t.name, MAX(s.start_at_ms) AS last_used
FROM tag t JOIN segment_tag st ON st.tag_id = t.id
JOIN time_segment s ON s.id = st.segment_id
GROUP BY t.id ORDER BY last_used DESC LIMIT 8;
```

冲突检测（半开区间）：

```sql
SELECT * FROM time_segment
WHERE status IN ('active','closed')
  AND id <> :exclude_id
  AND start_at_ms < :new_end
  AND (end_at_ms IS NULL OR end_at_ms > :new_start);
```

---

## 7. 导出格式

### 7.1 JSON

```json
{
  "schema_version": 1,
  "app": { "name": "dotlog", "version": "0.1.0" },
  "exported_at": "2026-10-28T22:00:00+08:00",
  "data": {
    "segments": [
      {
        "id": "0192...",
        "start_at": "2026-10-28T09:00:00+08:00",
        "end_at": "2026-10-28T10:12:00+08:00",
        "title": "高数 880 题 21-35",
        "note": "",
        "category": "学习",
        "tags": ["高数"],
        "is_focus": true,
        "is_ignored": false,
        "source": "hotkey"
      }
    ],
    "categories": [],
    "tags": [],
    "settings": {}
  }
}
```

导入：先校验 `schema_version ≤ 当前` 与字段合法性；建立安全副本后写入；冲突段默认跳过并报告。

### 7.2 CSV

列：`date,start,end,duration_min,title,category,tags,note,is_focus,is_ignored,source`
编码 UTF-8 with BOM（Excel 兼容），时间用本地 ISO 8601。

---

## 8. 备份与恢复

- 触发：应用启动后 5 分钟、每天首次使用、手动“立即备份”。
- 方式：`VACUUM INTO 'backups/dotlog-YYYYMMDD-HHmmss.db'`（在线一致快照）。
- 保留：最近 7 个日备份、4 个周备份、6 个月备份；超出按策略清理。
- 校验：备份完成后 `PRAGMA integrity_check`，失败则删除并记录日志。
- 恢复：先对当前库做安全副本 → 校验目标库 → 关闭连接 → 原子替换 → 重启迁移检查。

---

## 9. 完整性自检（启动时）

1. `PRAGMA quick_check`。
2. 查询 active 段数量必须是 0 或 1。
3. 相邻段重叠检测（按 `start_at_ms` 排序扫描）。
4. 段时长异常（负值、> 72 h 未恢复）生成提示。
5. 结果写入诊断日志；仅重叠/损坏触发用户可见提示。

---

## 10. 二期预留（迁移 0002，不入 MVP）

```sql
CREATE TABLE ai_suggestion (
  id             TEXT PRIMARY KEY,
  segment_id     TEXT REFERENCES time_segment(id) ON DELETE CASCADE,
  provider       TEXT NOT NULL,
  kind           TEXT NOT NULL,        -- classify / summary / query
  content        TEXT NOT NULL,
  state          TEXT NOT NULL DEFAULT 'proposed', -- proposed/accepted/rejected
  created_at_ms  INTEGER NOT NULL,
  resolved_at_ms INTEGER
) STRICT;
```

原则：AI 结果只进 `ai_suggestion`，用户确认后才写入 `time_segment`；原始记录永不被覆盖。
