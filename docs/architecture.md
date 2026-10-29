# 打点 DotLog · 技术架构设计

版本：v0.1 设计稿 ｜ 日期：2026-10-28 ｜ 适用：Windows 首发，代码保持跨平台

---

## 1. 目标与约束

1. 记录动作足够快：一次打点 ≤ 5 秒完成（含补记）。
2. 时间线可信：任何异常关闭后已落盘记录不丢失，当天总时长与各段求和误差为 0。
3. 本地优先：不开启任何集成时，软件完全离线可用。
4. 核心稳定、扩展开放：核心计时不依赖 UI、网络、数据库实现或任何插件。
5. 数据可掌握：SQLite 单文件 + 可读导出 + 可恢复备份。

约束：单用户、单设备、单进程；不做团队、账号、云同步、手机端、默认窗口监控、生产力评分。

---

## 2. 工具链（已在本机核验）

| 组件 | 版本 / 状态 | 说明 |
|---|---|---|
| Node.js | 24.16.0 | 前端构建 |
| npm | 11.13.0 | 包管理器（本机未装 pnpm，统一用 npm） |
| Rust | 1.99.1 (x86_64-pc-windows-msvc) | MSVC linker 实测可编译 |
| WebView2 | 156.0.4314.50 | Tauri 2 渲染运行时 |
| SQLite | 由 `rusqlite` 的 `bundled` 特性内置 | 版本随二进制固定，避免系统差异 |

---

## 3. 分层与职责

```
apps/desktop (React + TS)
        │  typed IPC（Rust 生成 TS）
apps/desktop/src-tauri (Tauri 2：托盘 / 快捷键 / 单实例 / 通知 / 命令路由)
        │
crates/application (用例、事务边界、领域事件、端口 trait)
        │
crates/domain (状态机、不变量、统计口径；零外部依赖)
        ▲
crates/storage (SQLite 仓储、迁移、备份)  ├─ crates/plugin-sdk
                                          ├─ crates/plugin-host
                                          └─ crates/integrations/*
```

### 3.1 模块职责与依赖规则

| 模块 | 职责 | 允许依赖 | 硬性禁止 |
|---|---|---|---|
| `domain` | 状态机、转换规则、不变量、统计公式、跨日切片 | 仅 `serde` / 时间类型（时间以 i64 毫秒传入） | 数据库、网络、UI、OS API、async runtime、日志实现 |
| `application` | 用例编排、事务边界、查询组装、领域事件、端口定义 | `domain` | 具体 storage 实现、Tauri、插件实现 |
| `storage` | 仓储实现、迁移执行、备份/恢复、导出 | `application`（port）、`rusqlite` | UI、插件 |
| `plugin-sdk` | manifest、能力、扩展点类型、`Plugin` trait | `domain` DTO | 任何具体实现 |
| `plugin-host` | 插件发现、启停、权限裁决、隔离与超时、outbox 调度 | `plugin-sdk`、`application` 公开服务 | 直接持有数据库连接 |
| `integrations/*` | Memos、AI、导入导出适配器 | `plugin-sdk` | 侵入计时逻辑 |
| `apps/desktop/src-tauri` | 系统能力封装、命令路由、事件推送 | `application` | 写 SQL、写业务规则 |
| `packages/ui` | 组件、时间线、图表、主题 | 生成的 IPC 类型 | 直连 DB、复算统计 |
| `packages/ipc` | 生成的绑定与薄封装 | `@tauri-apps/api` | 手写与 Rust 不一致的类型 |

`domain` 的 `Cargo.toml` 只允许 `serde`、`thiserror` 与（可选）`chrono`；CI 检查 domain 的依赖树，出现 `rusqlite`/`reqwest`/`tauri` 即失败。

---

## 4. 仓库结构

```
dotlog/
├── apps/
│   └── desktop/
│       ├── package.json
│       ├── index.html
│       ├── src/                 # React 入口、路由、页面装配
│       │   ├── app/             # 路由、布局、查询客户端
│       │   ├── features/        # today / history / analytics / integrations / settings
│       │   └── shared/          # 主题、hooks、格式化
│       └── src-tauri/
│           ├── Cargo.toml
│           ├── tauri.conf.json
│           └── src/
│               ├── main.rs
│               ├── commands/    # timer / segment / day / stats / settings / recovery / backup / integration
│               ├── platform/    # tray / hotkey / notification / autostart / single_instance
│               └── events.rs
├── crates/
│   ├── domain/                  # 纯逻辑 + 单元测试
│   ├── application/             # 用例 + ports + 事件
│   ├── storage/                 # SQLite 仓储 + migrations/*.sql + backup
│   ├── plugin-sdk/
│   ├── plugin-host/
│   └── integrations/
│       ├── memos/
│       ├── ai/
│       └── exporter/
├── packages/
│   ├── ui/                      # 组件、时间线、SVG 图表、tokens
│   └── ipc/                     # 生成的 bindings
├── plugins/                     # 内置集成（编译进 host 的注册表）
└── docs/                        # 本目录：架构、领域、数据、契约、SDK、UI、路线
```

Cargo workspace：根 `Cargo.toml` 统一版本与 lint；`apps/desktop/src-tauri` 与各 crate 均为 workspace member。前端使用 npm workspaces（`apps/*`、`packages/*`）。

---

## 5. 运行时拓扑

- 单进程：Tauri 应用进程内嵌 WebView2。
- 线程模型：
  - 主线程：窗口与托盘事件。
  - Tokio runtime（Tauri 内置）：IPC 命令、事件推送、outbox 调度、备份任务。
  - DB 工作线程：`rusqlite::Connection` 由 `Mutex` 持有，所有命令在 `spawn_blocking` 中执行；单写连接避免 `SQLITE_BUSY` 竞争。
- 数据库：`PRAGMA journal_mode=WAL`、`foreign_keys=ON`、`busy_timeout=5000`、`synchronous=NORMAL`。
- 单实例：Tauri single-instance 插件；第二实例唤起主窗口并转发 `punch`（若用户开启“二次启动即打点”）。

---

## 6. 关键数据流：一次打点（默认 stop 模式）

```
快捷键 Ctrl+Alt+Space
  → desktop::commands::timer_punch
  → AppService::punch(now, source)
      BEGIN IMMEDIATE
        SELECT active segment（唯一索引保证 ≤1）
        UPDATE 该段 end_at_ms=now, status='closed', revision+1
        INSERT time_point(kind='punch', segment_id=关闭段, occurred_at_ms=now)
        （chain 模式下再 INSERT 一条 active 段）
      COMMIT
  → domain 计算 TimerState / DayStats 增量
  → 返回 PunchResult { closedSegmentId, newSegmentId?, undoToken, shortSegment }
  → 前端弹出补记框（焦点在输入框，5 秒内可保存；Esc 跳过）
  → 事件 timer://changed 推送给托盘、主窗口
```

失败路径：事务回滚，active 段保持原样，返回结构化错误码；前端提示“本次打点未保存”，不丢失原记录。

---

## 7. 技术决策记录（ADR）

| 编号 | 决策 | 理由 | 影响与替代方案 |
|---|---|---|---|
| ADR-001 | Tauri 2 + React + TypeScript + Rust | 任务书指定；安装包小、系统能力可控、跨平台 | 替代 Electron：包体与内存更小；代价是 Rust 学习成本 |
| ADR-002 | `rusqlite` + `bundled` SQLite | 同步 API 简单、事务边界清晰、跨平台版本一致 | 替代 sqlx：无需异步驱动与编译期离线缓存 |
| ADR-003 | IPC 契约由 Rust 生成 TS（`tauri-specta`） | 单一事实来源，避免两侧 DTO 漂移 | 若 specta 不满足，降级为 `ts-rs` 生成类型 + 手写 invoke |
| ADR-004 | 计时权威在 Rust | 前端累加器会漂移、休眠后不可信 | 前端只按 `startAt` 做秒级显示，写入时间一律取后端 |
| ADR-005 | 打点为单事务 | 保证“关闭旧段 + 写事实 + 开新段”原子性 | 不允许分段提交 |
| ADR-006 | 默认 `stop` 模式，`chain` 可配置 | 用户确认：打点后回到空闲，避免忘停 | 设置项 `punch.mode = 'stop' \| 'chain'`，领域层同时支持 |
| ADR-007 | 跨日只做展示切片 | 原始记录保持完整，统计与编辑不歧义 | 切片算法在 domain，DB 不拆行 |
| ADR-008 | 统计只实现一次（`domain::stats`） | 避免前后端口径不一致 | 前端只渲染 ViewModel |
| ADR-009 | 插件不得直写核心表 | 保护数据完整性 | 插件只拿服务接口 + outbox |
| ADR-010 | 图表自绘 SVG | 体积小、可主题化、无第三方锁 | 替代 ECharts/Recharts；复杂图表后置 |

---

## 8. 跨平台系统能力

`src-tauri/src/platform/` 定义 trait，Windows 实现，为 macOS/Linux 预留：

```rust
trait SystemShell {
    fn set_tray(&self, state: TrayState) -> Result<()>;
    fn register_hotkey(&self, accel: &str) -> Result<()>;
    fn notify(&self, n: Notification) -> Result<()>;
    fn set_autostart(&self, enabled: bool) -> Result<()>;
}
```

- 托盘：左键打点、右键菜单（暂停 / 结束 / 打开主界面 / 退出）；标题显示当前段用时。
- 快捷键：默认 `Ctrl+Alt+Space`，注册失败时（冲突）在设置页给出明确提示并保留旧配置。
- 通知：默认轻提示，可在设置关闭。
- 关窗行为：隐藏到托盘，进程常驻；退出仅由托盘菜单或设置页触发。

---

## 9. 错误处理、日志与诊断

- 错误类型：`domain::DomainError`（纯）、`application::AppError`（带 code）。
- IPC 错误统一信封：`{ code, message, detail? }`；前端维护 code → 中文文案映射。
- 不在命令中使用 `unwrap`/`panic`；崩溃由 panic hook 写入 `crash.marker`，下次启动触发恢复核对。
- 日志：`tracing` + 每日滚动文件（`%APPDATA%/dotlog/logs/`），默认 info，敏感字段（token、备注全文）不落日志。
- `diagnostics_snapshot` 生成诊断包（版本、迁移版本、最近 200 行日志、表计数），供反馈使用。

---

## 10. 性能预算

| 指标 | 目标 |
|---|---|
| 冷启动到可打点 | < 1.5 s |
| 打点 IPC 往返（p95） | < 50 ms |
| 今日页加载（≤ 200 段） | < 100 ms |
| 单日查询（10,000 段历史库） | < 150 ms |
| 常驻内存 | < 150 MB |
| 年数据量（每天 30 段） | < 30 MB |

---

## 11. 安全与隐私

1. 所有业务数据仅存本机 SQLite；无遥测。
2. 令牌只进系统钥匙串（Windows Credential Manager），配置文件只存引用键名。
3. 联网能力逐插件授权，UI 显示“最近一次联网时间”。
4. 导出包含 `schema_version`；导入先校验并创建安全副本。
5. 迁移前自动备份；迁移失败保持旧库不变。

---

## 12. 测试与构建

- 测试分层见 `roadmap.md`。
- CI 门槛：`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`、`npm run typecheck`、`npm run test`、`npm run build`。
- 发布：`npm run tauri build` 生成 NSIS 安装包；自动更新在 M5 接入（先做签名与回滚验证）。

---

## 13. 与其他文档的关系

- 状态机与业务规则：`domain-model.md`
- 表结构与字段：`data-dictionary.md`
- 命令/事件/错误码：`ipc-contract.md`
- 插件与 Memos/AI：`plugin-sdk.md`
- 页面与组件：`ui-spec.md`
- 里程碑与验收：`roadmap.md`
