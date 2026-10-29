# 打点 DotLog · 领域模型与状态机

版本：v0.1 ｜ 依赖：无（纯逻辑）

---

## 1. 术语

| 术语 | 含义 |
|---|---|
| 时间点（time_point） | 一次打点事实，只追加、可标记撤销，不参与计时计算 |
| 时间段（segment） | 用户看到与编辑的记录，有起止、标题、标签；`active` 段无结束时间 |
| 打点（punch） | 关闭当前 active 段并记录事实；是否立刻开新段由模式决定 |
| 补记 | 对刚关闭的段补充标题 / 备注 / 标签 |
| 空白区间（gap） | 用户可记录时段内没有被任何段覆盖的时间 |
| 展示切片（slice） | 跨日段在某一天视图中的片段，保留原 `segment_id` |

---

## 2. 状态定义

持久化状态只有两相：

```rust
pub enum Phase { Idle, Recording }
```

派生状态（不落库，由数据推导）：

| 派生状态 | 条件 | UI 表现 |
|---|---|---|
| 未开始 | `Idle` 且当天无段 | 主按钮“开始打点” |
| 记录中 | `Recording` | 主按钮“打点”，显示当前段用时 |
| 暂停中 | `Idle` 且最后一个时间点 `kind='pause'` | 主按钮“继续”，提示空白区间 |
| 待补记 | 返回结果含 `pendingNote` | 补记框浮出，Esc 可跳过 |
| 跨日未结束 | `Recording` 且 active 段起始早于今天边界 | 启动/切日时要求处理 |
| 需恢复 | 启动检测到长间隔 / 孤儿 active / 时钟异常 | 阻塞式恢复对话框 |

暂停不是独立持久状态：暂停 = 关闭当前段 + 写入 `time_point(kind='pause')`，之后处于 `Idle`。这样避免“暂停中被崩溃”产生第三种不一致。

---

## 3. 打点模式（ADR-006）

设置项 `punch.mode`：

| 值 | 行为 | 默认 |
|---|---|---|
| `stop` | 打点后关闭当前段，回到 `Idle`；再点一次“开始打点”才进入下一段 | ✅ 用户已确认 |
| `chain` | 打点关闭当前段并立即创建下一段（起点 = 打点时刻） | 可配置 |

两种模式共享同一套不变量、撤销与统计逻辑，差异只在事务内是否插入新的 active 段。

---

## 4. 命令与状态转换

命令签名（domain 层，时间由 application 传入）：

```rust
fn start(state: &TimerState, now: i64, source: PointSource) -> Result<Transition>;
fn punch(state: &TimerState, now: i64, mode: PunchMode, source: PointSource) -> Result<Transition>;
fn pause(state: &TimerState, now: i64, source: PointSource) -> Result<Transition>;
fn resume(state: &TimerState, now: i64, source: PointSource) -> Result<Transition>;
fn finish(state: &TimerState, now: i64, source: PointSource) -> Result<Transition>;
fn undo(state: &TimerState, now: i64, token: UndoToken) -> Result<Transition>;
fn update_segment(seg: &Segment, patch: SegmentPatch, now: i64) -> Result<Vec<Change>>;
fn backfill(input: BackfillInput, existing: &[Segment], now: i64) -> Result<Vec<Segment>>;
fn delete(seg: &Segment, now: i64) -> Result<Change>;
```

| # | 当前 | 命令 | 结果 | 说明 |
|---|---|---|---|---|
| T1 | Idle | `start` | 新建 active 段（`title=''`），→ Recording | 起点 = now |
| T2 | Idle（暂停后） | `resume` | 同 T1，时间点 `kind='resume'` | UI 文案“继续” |
| T3 | Recording | `punch` + `stop` | 关闭 active（`end=now`），写 `punch` 事实，→ Idle | 返回 `pendingNote` |
| T4 | Recording | `punch` + `chain` | 关闭 active，写 `punch`，再建新 active，→ Recording | 返回 `pendingNote` |
| T5 | Recording | `finish` | 关闭 active，写 `finish`，→ Idle | 当天结束；仍允许补记 |
| T6 | Recording | `pause` | 关闭 active，写 `pause`，→ Idle | 空白区间从此开始 |
| T7 | Idle | `punch` / `finish` / `pause` | `E_NOT_RECORDING` | 不写任何数据 |
| T8 | Recording（异常孤儿） | `start` | `E_ACTIVE_EXISTS` | 必须先恢复或结束 |
| T9 | 任意 | `undo`（有效 15 s） | 见 §7 | 只允许最近一次事实 |
| T10 | Recording | `update_segment` | 段属性变化，phase 不变 | 改起点需 < now 且不与前段重叠 |
| T11 | Idle | `backfill` | 插入一条已关闭段 | 起止必填，冲突则拒绝 |
| T12 | 任意 | `delete` | 软删除，写 `delete` 事实 | active 段禁止直接删除 |

### 4.1 打点事务（默认 stop 模式）

```text
BEGIN IMMEDIATE
  active ← SELECT * FROM time_segment WHERE status='active'       -- 唯一索引保证 ≤1
  IF active IS NULL → ROLLBACK, E_NOT_RECORDING
  IF now <= active.start_at_ms → ROLLBACK, E_INVALID_RANGE
  UPDATE time_segment
     SET end_at_ms = now, end_tz_offset_min = tz(now),
         status = 'closed', revision = revision + 1, updated_at_ms = now
   WHERE id = active.id
  INSERT INTO time_point(id, occurred_at_ms, tz_offset_min, kind='punch',
                         source, segment_id=active.id)
  IF mode = 'chain':
      INSERT time_segment(id=new, start_at_ms=now, end_at_ms=NULL,
                          status='active', title='', revision=1)
COMMIT
```

`finish` / `pause` 与上相同，只是不插入新段、`kind` 不同。任何一步失败整体回滚。

---

## 5. 不变量

| 编号 | 不变量 | 保障方式 |
|---|---|---|
| I1 | 任意时刻最多一个 `active` 段 | `CREATE UNIQUE INDEX ux_segment_single_active ... WHERE status='active'` + 事务内检查 |
| I2 | 非 active 段必须 `end_at_ms > start_at_ms` | DDL CHECK + domain 校验 |
| I3 | 普通（非删除）段之间不重叠（半开区间 `[start, end)`） | application 写前查询冲突；编辑/补录同样校验 |
| I4 | `active` 段 `end_at_ms IS NULL` | DDL CHECK |
| I5 | 删除只做软删除并更新 `updated_at_ms` | 仓储 API 不暴露物理删除（导入修复除外） |
| I6 | 每次写入 `revision + 1` | 为未来同步/冲突检测预留 |
| I7 | `time_point` 只追加；撤销通过 `undone_at_ms` 标记 | 审计与崩溃排查 |
| I8 | 段内时间使用 UTC 毫秒；显示用记录的时区偏移 | 夏令时/跨时区安全 |

---

## 6. 时间与日历规则

1. 所有时间以 **UTC epoch 毫秒** 存储；每条段与时间点保存 `tz_offset_min`（记录当时的本地时区偏移）。
2. 展示日期由 `local_date = floor((utc_ms + tz_offset_min*60_000 - day_start_offset) / 86_400_000)` 计算，`day_start_minutes` 默认 **240（04:00）**，可配置。
3. 一天窗口为 `[边界, 边界+24h)`；今天的窗口末端取 `now`（不把未来算成空白）。
4. 夏令时导致某天为 23/25 小时：以实际毫秒计算，不做补偿。
5. 系统时钟跳变检测：进程内维护 `wall_now` 与单调时钟差；启动时对比 `runtime_state.heartbeat`（每 30 s 写入 `wall_ms` 与 `mono_ms`）。若向后跳变 > 2 s 或心跳断档 > 5 min，产生恢复提示。

---

## 7. 撤销

- 撤销窗口：`punch.undo_window_ms` 默认 **15 000 ms**；仅允许撤销最近一个 `undone_at_ms IS NULL` 的时间点，且其后无其他事实。
- 逆操作按 `kind` 执行：

| kind | 逆操作 |
|---|---|
| `punch`（stop） | 重开被关闭段（`end_at_ms=NULL, status='active'`）；无新段需删除 |
| `punch`（chain） | 重开被关闭段；软删除打点瞬间创建的下一段（若已有标题/标签则拒绝并提示改用时间线编辑） |
| `pause` / `finish` | 重开被关闭段为 active |
| `start` / `resume` | 删除刚创建的空 active 段，回到 Idle |
| `delete` | 恢复被删段 |
| `edit` / `backfill` | 恢复 `payload` 中的旧值 |

- 撤销本身写一条 `kind='undo'` 的事实，`payload` 记录 `undo_of=<time_point.id>`，便于审计。
- 超过窗口后 Ctrl+Z 不生效，UI 提示“请到时间线直接修改”。
- 短段提示：关闭段时长 < `punch.short_segment_ms`（默认 15 s）时，`PunchResult.shortSegment=true`，UI 显示“刚记录 8 秒，撤销这次打点？”。

---

## 8. 异常与恢复

启动流程（application `startup_reconcile`）：

```text
1. 打开 DB，执行迁移（先备份）
2. 读取 active 段与最近 time_point、heartbeat
3. IF active 存在:
     a. 起始早于今天边界 → RecoveryRequired(CrossDay)
     b. 心跳断档 > long_gap_minutes（默认 5）→ RecoveryRequired(LongGap)
     c. 当前时间 < start_at → RecoveryRequired(ClockAnomaly)
4. ELSE IF 时钟异常 → 仅提示，不阻塞
5. 正常 → 进入 Idle/Recording
```

恢复对话框选项（`recovery_resolve`）：

| 场景 | 选项 |
|---|---|
| 跨日未结束 | 截断到昨日边界 / 延续到当前时刻 / 丢弃该段 |
| 长间隔（休眠、关机） | 计入这段 / 截断到间隙开始前 / 丢弃 |
| 时钟异常 | 以当前时间为准结束 / 保留并稍后手动修改 |

处理完成后写 `time_point(kind='edit'/'finish', source='system', payload={reason})`。

其它容错：

- **区间重叠**：编辑/补录前预检，返回冲突段列表；UI 提供“裁剪前一段”“拆分”“取消”。保存阶段仍由 application 二次校验。
- **跨午夜**：不拆行，视图切片；`day_get` 返回片段并带 `segmentId` 供跳转编辑。
- **数据损坏**：打开时 `PRAGMA quick_check`，失败则提示从最近备份恢复，禁止静默重建。

---

## 9. 编辑与补录规则

| 操作 | 规则 |
|---|---|
| 修改标题/备注/标签/分类 | 任意状态允许；写 `revision+1` |
| 修改起点 | 必须 `< now` 且 `< end`，且不早于前一段结束；active 段允许 |
| 修改终点 | 必须 `> start` 且不晚于下一段起点；active 段不可设终点（用结束命令） |
| 补录 | 起止均必填、跨度 ≤ 24 h、不与已有段重叠；可跨日（显示时分片） |
| 删除 | 软删除；若删除 active，先以 now 关闭再删除，并写两条事实 |
| 标签 | 多对多；同名标签复用（大小写不敏感） |

---

## 10. 统计口径（唯一实现：`domain::stats`）

输入：某日窗口 `[day_start, day_end)`、段列表、设置（focus 阈值、可记录时段）。

```text
intersect(seg, day) = max(0, min(seg.end, day_end) - max(seg.start, day_start))
```

| 指标 | 公式 | 说明 |
|---|---|---|
| 记录总时长 | Σ intersect(seg, day)，seg 非删除 | 跨日按交集 |
| 有效记录时长 | Σ intersect，条件：`title != '' OR category_id != null`，且 `is_ignored = 0` | 空标题且无分类不算有效 |
| 空白时长 | `max(0, 可记录窗口长度 - 记录总时长)` | 今天窗口末端取 now |
| 专注时长 | Σ intersect，条件：`is_focus = 1` 且段时长 ≥ `focus.min_minutes`（默认 10） | 阈值按整段时长判断 |
| 记录完整率 | `有效时长 / 可记录窗口长度`（上限 100%） | 无窗口配置时用全天 |
| 切换次数 | 有效段数量 − 1（最小 0） | 暂停段、忽略段不计 |
| 段数 / 平均段长 | 非删除段计数 / 均值 | 用于回顾页 |

规则：所有报表数字必须可点击下钻到组成它的段集合；MVP 不引入“生产力评分”。

---

## 11. 领域事件

```rust
pub enum DomainEvent {
    SegmentClosed { segment: Segment, reason: CloseReason },
    SegmentUpdated { segment: Segment, changed: Vec<Field> },
    SegmentDeleted { segment_id: String },
    TimerChanged { state: TimerState },
    DayInvalidated { local_date: String },
}
```

- `SegmentClosed` 是插件唯一触发点（`onSegmentClosed`）。
- `DayInvalidated` 用于前端失效当天缓存；跨日切片变化时发出。
- 事件在事务提交后发布，发布失败不影响写入。

---

## 12. 测试用例清单（domain 层，FakeClock）

| 用例 | 断言 |
|---|---|
| 开始 → 打点（stop） | 关闭段 end=now；phase=Idle；生成 punch 事实；无新 active |
| 开始 → 打点（chain） | 关闭旧段；新 active 起点=now；时长连续 |
| 连续两次打点间隔 < 15 s | `shortSegment=true`；撤销后仅剩原段 |
| 暂停 → 继续 | 空白区间出现在两段之间；resume 事实正确 |
| 打点时机早于段起点 | 返回 `E_INVALID_RANGE`，不写库 |
| 编辑终点早于起点 | 拒绝 |
| 补录与已有段重叠 | 返回冲突列表；不写入 |
| 补录与前段首尾相接 | 允许（半开区间） |
| 跨午夜段切片 | 昨日/今日片段之和 == 原段时长 |
| 跨午夜 + 统计 | 两天记录总时长等于交集之和，不重复 |
| 夏令时切换 | 本地日窗口长度 23h，统计按实际毫秒 |
| 撤销 punch 后再次打点 | 同一段可再次关闭，事实链正确 |
| 撤销窗口过期 | 拒绝撤销，段保持关闭 |
| 删除 active | 先关闭再软删，active 索引释放 |
| 空标题段统计 | 计入记录总时长，不计有效时长 |
| 专注阈值边界 | 9:59 不计，10:00 计入 |
| 长间隔恢复（计入） | 段终点=now，事实 kind=system |
| 长间隔恢复（截断） | 段终点=间隙开始，空白保留 |
| 时钟回拨 | 恢复提示，阻塞写入直到选择 |
| 一万段查询 | 统计与逐段求和一致 |

---

## 13. 与其它层的边界

- domain 不读取 `setting` 表：所有可配置项以参数对象传入（`Policy { day_start_minutes, focus_min_minutes, ... }`）。
- domain 不生成 ID：ID 由 application 注入，便于测试与确定性。
- domain 不写事件总线：返回 `Vec<DomainEvent>`，由 application 发布。
