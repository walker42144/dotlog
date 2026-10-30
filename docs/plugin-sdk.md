# 打点 DotLog · 插件 SDK 与集成设计（v1）

> 本文描述内置Rust集成SDK。可从文件安装的UI Worker插件请阅读 [本地插件编写规范](local-plugin-guide.md)，两种清单格式不同。

原则：**核心不需要任何插件也能长期稳定运行**。插件只能“消费事实、提出建议、发送外部数据”，不能触碰计时核心表。

---

## 1. 插件形态

| 阶段 | 形态 | 说明 |
|---|---|---|
| v1（MVP） | 内置插件（编译进 `plugin-host` 注册表） | 先稳定契约，随安装包发布 |
| v2 | 外部插件（独立可执行 sidecar + manifest） | 进程隔离，权限声明，用户手动安装 |

文档中的接口在 v1 就按外部插件标准设计，避免二期重写。

---

## 2. Manifest

```json
{
  "id": "memos",
  "name": "Memos 发布",
  "version": "1.0.0",
  "apiVersion": 1,
  "capabilities": ["segments:read", "network", "secret:store", "ui:panel"],
  "settingsSchema": { "$ref": "#/definitions/memos" },
  "entry": "builtin:memos"
}
```

字段规则：

| 字段 | 规则 |
|---|---|
| `id` | 小写 + 连字符，全局唯一，安装后不可更改 |
| `apiVersion` | 与 `plugin_sdk::API_VERSION` 严格匹配，不兼容则拒绝加载 |
| `capabilities` | 见 §3，运行时逐项裁决 |
| `settingsSchema` | JSON Schema 子集（string/number/boolean/enum），用于自动生成设置表单 |
| `entry` | v1 为 `builtin:<id>`；v2 为可执行文件相对路径 |

---

## 3. 能力（Capability）

| 能力 | 允许访问 | 典型用途 |
|---|---|---|
| `segments:read` | 只读段查询（按日期/ID） | AI 分类、报表 |
| `tags:write` | 向段追加标签（需用户确认或规则授权） | 自动打标签 |
| `network` | 通过 host 提供的 HTTP 客户端访问声明域名 | Memos、AI |
| `clipboard:read` | 读取剪贴板文本（一次性，用户触发） | 快速补记 |
| `ui:panel` | 在集成页/详情页显示面板 | 同步状态 |
| `secret:store` | 通过钥匙串读写本插件密钥 | Token |
| `importer` / `exporter` | 注册导入/导出器 | ActivityWatch、CSV、Markdown |

约束：

- 未声明或未授权的能力，对应上下文对象为 `None`，调用即失败，不靠“自觉”。
- 联网域名在 manifest 中显式声明（`networkHosts`），host 做白名单校验。
- `secret:store` 只允许读写 `dotlog.<pluginId>.*` 命名空间。
- 插件不得获得 SQL 连接、文件系统全盘访问或 Tauri 命令通道。

---

## 4. Rust 契约

```rust
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: u32,
    pub capabilities: Vec<Capability>,
    #[serde(rename = "settingsSchema")]
    pub settings_schema: serde_json::Value,
    pub entry: String,
}

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    /// 启动时调用一次；失败则插件标记为不可用，不影响应用启动。
    fn init(&mut self, ctx: &PluginCtx) -> Result<(), PluginError>;

    /// 时间段关闭后的异步处理（唯一的核心事件入口）。
    fn on_segment_closed(&mut self, ctx: &PluginCtx, event: &SegmentClosedEvent) -> Result<(), PluginError> {
        Ok(())
    }

    /// 注册命令与面板（v1 仅内置插件支持）。
    fn commands(&self) -> Vec<PluginCommand> { Vec::new() }

    fn shutdown(&mut self) {}
}

pub struct SegmentClosedEvent {
    pub segment: SegmentSnapshot,   // 关闭瞬间的不可变快照
    pub reason: CloseReason,        // punch / finish / system_recovery
}

pub struct PluginCtx<'a> {
    pub plugin_id: &'a str,
    pub segments: Option<&'a dyn SegmentQuery>,   // segments:read
    pub tags: Option<&'a dyn TagWriter>,          // tags:write
    pub http: Option<&'a dyn HttpClient>,         // network
    pub secrets: Option<&'a dyn SecretStore>,     // secret:store
    pub outbox: &'a dyn OutboxSink,               // 始终可用（本地队列）
    pub settings: &'a dyn PluginSettings,         // 只读本插件配置
    pub log: &'a dyn PluginLogger,
}
```

关键点：

- 事件传递 **快照**，插件延迟执行时读取到的不是最新可变状态，避免竞态。
- 插件写标签必须走 `TagWriter`，由 host 校验能力并写审计记录。
- 重试与补偿统一交给 outbox，插件不自己实现重试循环。

---

## 5. 生命周期与隔离

```text
发现（注册表） → 校验 manifest/apiVersion → 能力裁决
  → init(ctx) [超时 3 s，失败则禁用]
  → 运行：on_segment_closed [超时 3 s] / 手动命令 [超时 10 s]
  → 用户禁用 / 应用退出 → shutdown [超时 2 s]
```

- 所有插件调用 `spawn_blocking` + `tokio::time::timeout`；超时记日志并跳过，不阻塞打点事务。
- 捕获 panic（`catch_unwind`），单次失败只影响该插件。
- 同一插件的事件串行执行，避免顺序错乱。
- 插件崩溃不会导致 active 段丢失（打点事务已提交）。

---

## 6. Outbox：可靠异步发送

```text
punch 事务提交 → application 发布 SegmentClosed
  → plugin-host 调用插件 on_segment_closed
  → 适配器写入 outbox_event(dedupe_key='memos:<segmentId>:publish')
  → worker（每 30 s tick）取 due 事件 → 发送 → 成功标记 done / 失败退避
```

| 项 | 规则 |
|---|---|
| 去重 | `dedupe_key` 唯一；重复入队直接忽略 |
| 退避 | 30 s → 2 min → 10 min → 1 h → 6 h，最多 6 次 |
| 原子性 | 先写 outbox 再返回；断网不影响本地记录 |
| 幂等 | 发布成功后写 `integration_link.external_id`，重复发送前先查 |
| 状态 | `pending` / `inflight`（带 5 min 租约，超时回收）/ `done` / `failed` |
| 手动 | 集成页可查失败原因并“立即重试” |

---

## 7. Memos 适配器（首个集成）

### 7.1 配置

| 项 | 说明 |
|---|---|
| `instanceUrl` | 用户实例地址，校验 https 或局域网 http |
| `token` | 个人访问令牌，存系统钥匙串 `dotlog.memos.token` |
| `visibility` | `private` / `protected` / `public`，默认 `private` |
| `autoPublish` | `off` / `on_close` / `tag_rule`，默认 `off` |
| `template` | Markdown 模板，见 §7.3 |
| `apiVersion` | 连接测试时探测并保存（如 `v1` / `v2`） |

### 7.2 版本适配

Memos 不同版本 API 路径与字段存在差异。适配器在 `integration_test` 时按顺序探测：

```text
1. GET {base}/api/v1/status      → 200 则记住 v1
2. GET {base}/api/v1/workspace/profile → 200 则备选
3. 失败则提示用户检查地址/令牌或填写 API 前缀
```

发送时只使用已探测版本的路径封装；不硬编码单一版本。所有路径集中在 `adapters/memos/endpoints.rs`，便于后续修补。

### 7.3 发布格式

```markdown
**09:00–10:12（1h12m）** 高数 880 题 21–35
#学习 #高数
<!-- dotlog:segment=0192... -->
```

- 首版**单向发布**：本地 → Memos，不做回链同步，避免冲突。
- 末尾隐藏注释保存 `segment_id`，便于幂等与未来反查。
- 若段后续被编辑，默认不重新发布；设置项可开启“编辑后更新”（二期）。
- 发布成功后写 `integration_link(provider='memos', external_id, state='published')`。

### 7.4 隐私

- 发送前可配置“只发标题，不发备注”。
- 集成页展示“本次将发送的字段”，首次发布需确认。

---

## 8. AI 适配器

### 8.1 Provider 接口

```rust
pub trait AiProvider: Send + Sync {
    fn id(&self) -> &'static str;              // openai | ollama | custom
    fn classify(&self, input: ClassifyInput) -> Result<Vec<AiSuggestion>>;
    fn summarize_day(&self, input: SummaryInput) -> Result<AiSuggestion>;
    fn query(&self, question: &str, scope: QueryScope) -> Result<AiSuggestion>;
}
```

- 支持 OpenAI 兼容端点与本地 Ollama；模型名、baseURL 由用户配置。
- 请求超时 20 s，失败不影响本地任何数据。

### 8.2 事实与建议分离

- AI 输出只写 `ai_suggestion`（二期表），状态 `proposed`。
- UI 以“建议”呈现，用户接受后才写 `title` / `tags` / `category_id`。
- 原始记录永不被 AI 覆盖；所有接受操作写入 `time_point(kind='edit', source='plugin')`。

### 8.3 隐私控制

| 模式 | 发送内容 |
|---|---|
| `title_only`（默认） | 标题 + 起止 + 分类/标签 |
| `with_note` | 追加备注全文 |
| `off` | 不联网 |

首次开启时展示数据范围与提供商名称。

---

## 9. 导入 / 导出器

| 插件 | 方向 | 格式 | 说明 |
|---|---|---|---|
| `exporter-csv` | 导出 | CSV | UTF-8 BOM，列定义见数据字典 |
| `exporter-markdown` | 导出 | Markdown 日报 | 时间线 + 分类统计（M2） |
| `importer-csv` | 导入 | CSV | 预览 → 冲突报告 → 确认写入 |
| `importer-timewarrior` | 导入 | JSON 导出 | 映射 interval → segment |
| `importer-activitywatch` | 导入 | 只读本地库 | 仅导入聚合结果，不默认采集窗口标题 |

导入统一流程：解析 → 校验 → 预览（含冲突）→ 安全备份 → 事务写入 → 报告。

---

## 10. 插件设置命名空间

- 表：`setting(scope='plugin:<id>', key, value)`。
- 由 manifest 的 `settingsSchema` 自动生成表单；host 在保存前校验类型。
- 密钥不进入该表：`secret:store` → 系统钥匙串。
- 导出数据默认**不含**密钥与插件令牌。

---

## 11. SDK 版本策略

| 变更 | 处理 |
|---|---|
| 新增可选字段 / 新扩展点 | `apiVersion` 不变，旧插件继续工作 |
| 修改 trait 方法签名 | `API_VERSION + 1`；旧插件禁用并提示升级 |
| 移除能力 | 需大版本；迁移期保留只读兼容层 |

---

## 12. 验收标准（v1）

1. 内置示例插件可独立启停，禁用后核心打点不受影响。
2. 撤销 `network` 能力后，插件发起请求立即失败且不弹系统网络错误。
3. 断网时打点正常，outbox 累积；恢复网络后自动补发且不重复。
4. 插件超时/panic 不影响主流程，日志含插件 id 与耗时。
5. 令牌仅存在钥匙串；配置文件与导出文件中搜不到明文。
