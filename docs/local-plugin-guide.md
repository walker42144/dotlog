# DotLog 本地插件编写规范 · API 1

本地插件可以提供独立页面、复盘工具、统计分析和表单。安装后由用户启用，可选择是否显示在侧边栏，也可从插件管理中打开。

## 1. 安装、更新与移除

打开“插件管理 → 从本地安装”，选择 `.dotlog-plugin.json` 文件。核对作者、版本和权限，确认安装，再打开“启用插件”。勾选“侧边栏入口”后出现入口。修改显示偏好、隐藏、停用、移除均立即生效。

同ID更新须勾选“替换同ID插件”。更新保留插件数据，但新版本默认停用，须重新确认权限并启用。移除会停止插件并隐藏入口，保留数据以便重新安装；不会删除计时记录。插件包中的作者字段是作者自述，不是签名认证，只安装可信来源的包。

## 2. 源码目录和打包

仓库 `examples/plugins/daily-review/` 提供可运行的每日复盘示例：

```text
daily-review/
  manifest.json
  plugin.js
```

复制此目录，修改清单与脚本。在仓库根目录运行：

```sh
node scripts/package-plugin.mjs examples/plugins/daily-review daily-review.dotlog-plugin.json
```

打包工具检查脚本语法，不执行插件。生成的单文件JSON包可直接本地安装，不需要重编译DotLog。包内字段如下：

```json
{
  "id": "local-daily-review",
  "name": "每日复盘",
  "version": "1.0.0",
  "apiVersion": 1,
  "description": "汇总所选日期的记录并保存复盘",
  "author": "你的名字",
  "permissions": ["segments:read", "storage"],
  "script": "DotLog.render({title:'你好',blocks:[{type:'text',text:'插件已运行'}]});"
}
```

ID必须以 `local-` 开头，仅含小写字母、数字、连字符，长度3至64；版本为三段数字。包上限256KB，最多安装50个。API版本严格为1；不支持的字段和权限会被拒绝。升级时保持ID不变，增加版本号。

## 3. 权限和运行方式

| 权限 | 允许的操作 |
| --- | --- |
| 无权限 | 本地计算和展示页面 |
| `segments:read` | 按日期读取记录，包括标题、备注、分类和标签 |
| `storage` | 读取和保存本插件自己的JSON数据，每插件64KB |
| `tomatodo:link`（宿主0.1.7起） | 调用受限番茄联动接口：启停后台、读关联状态、独立分段、确认备注和核对截止时间 |
| `tomatodo:history`（宿主0.1.15起） | 读取固定来源的历史数据、增量导入、对比；AI 分析还需另行启用 AI 助手 |

脚本在页面打开时进入独立Worker运行，离开页面、停用或移除时销毁。不是后台任务或原生程序；没有DOM、任意Tauri命令通道、SQL连接、系统文件、令牌或网络发布能力。0.1.7的 `tomatodo:link` 只能通过宿主校验的关联用例修改分段，不能注入来源快照或操作无关记录。后台观察属于宿主，切换页面不会停止；停用/移除/更新活动插件须先处理当前分段。生产版CSP阻止远程网络请求。Worker不等于完整的恶意代码安全边界，仍须选择可信作者。

首次显示5秒无响应会停止Worker，用户可重新加载。按钮操作最长等待30秒；SDK通常10秒超时，历史AI分析30秒。页面最多100个组件、总数据256KB；每秒最多60条Worker消息，同时最多4个SDK请求。安装不执行脚本。恢复备份后所有插件均停用，用户须重新启用。

## 4. JavaScript SDK

脚本是普通JavaScript，不支持ES模块导入、npm包解析或浏览器全局 `window`。自行把依赖打包到脚本中；无需React。

```js
DotLog.today();                       // 本地日期，例如2026-10-29
await DotLog.link.status();           // 以下link方法均需tomatodo:link
await DotLog.link.enable();           // 显式启用宿主后台
await DotLog.link.disable();          // 当前分段未结束会拒绝
await DotLog.link.startNext();
await DotLog.link.endSegment();
await DotLog.link.saveNote({id,revision,note});
await DotLog.link.skipNote({id,revision});
await DotLog.link.reconcile({endAt});  // 用户确认的UTC毫秒截止时间；不会更改已关闭段
await DotLog.getDay('2026-10-29');      // 需要segments:read
await DotLog.storage.get();           // JSON对象或null
await DotLog.storage.set({note:'复盘'}); // 覆盖本插件的数据
DotLog.render({ title: '我的工具', blocks: [] });
DotLog.onAction(async (action, values) => {
  if (action === 'save') {
    await DotLog.storage.set({note: values.note});
    DotLog.render({blocks:[{type:'text',text:'已保存'}]});
  }
});
```

`getDay` 返回 `{date, windowStart, windowEnd, segments, gaps, stats}`。`stats.recordedSeconds` 是按主程序当日边界计算的有效记录秒数。`segments` 的每项包含：`id/startAt/endAt/durationSeconds/title/note/category/tags/isFocus/isIgnored/status/source/revision`。时间为毫秒，时长为秒；`endAt` 可为空；`category` 可为空；`tags` 是含 `id/name/color` 的对象数组。

日期窗口遵循主程序的跨日设置，不要自行用结束时间减开始时间计算有效时长。API数据结构以 `apps/desktop/src/lib/ipc.ts` 为准。错误通过Promise抛出，异步逻辑应使用try/catch并给出可理解的提示。

## 5. 页面组件

`render` 替换页面描述，界面由DotLog的React渲染器生成，自动适配深浅主题。

| type | 字段 | 用途 |
| --- | --- | --- |
| `heading` | `text` | 小标题 |
| `text` | `text` | 普通文本 |
| `markdown` | `text` | Markdown及公式，不执行HTML |
| `stat` | `label`, `value` | 统计卡片，value为字符串 |
| `input` | `id`, `label`, `value`, `inputType` | 单行输入，inputType为text或date |
| `textarea` | `id`, `label`, `value` | 多行文本，最多20000字 |
| `button` | `text`, `action` | 向onAction发送动作及当前表单值 |

输入组件须提供唯一ID。按钮回调的 `values` 按输入ID索引，例如 `values.date`。重新render不会强制清空用户尚未提交的同ID输入；需要重置输入时改组件ID。禁止把用户数据拼成脚本执行，使用对象字段传值。

## 6. 调试和测试

先运行 `npm run dev --workspace @dotlog/desktop`，从插件管理安装本地包。浏览器预览使用模拟记录，插件包和插件数据存浏览器localStorage，与桌面数据库隔离。修改脚本后重新打包，明确选择替换更新并重新启用。

至少测试：不授权时不能读记录、跨日和暂停不重复累计、空数据、保存与重开、停用与移除后停止运行、同ID更新保留数据、错误和超时、明暗主题。桌面发布前还须测试WebView2环境与生产CSP，不要把浏览器预览通过等同于完整桌面验收。

## 7. 与原有Rust插件SDK的关系

`docs/plugin-sdk.md` 描述的是随主程序编译的Rust集成，提供Memos、AI等能力；本规范描述可本地安装的UI Worker插件。二者不能混用manifest或打包格式。不支持加载外部Rust动态库、任意可执行文件、MCP服务或任意插件后台钩子。`tomatodo:link` 是由宿主提供的单一受控后台，不会给插件开放原生进程执行或任意网络能力。联动插件示例在 `examples/plugins/tomatodo-link`。旧宿主会拒绝这个新权限，需要先更新宿主。

## 0.1.15 扩展：独立页面与历史数据

番茄联动 0.4.0 包的页面、实时/历史切换、状态文字与操作编排均在插件包内。DotLog 不再按旧番茄历史模块的开关创建同名入口；侧边栏严格取决于该本地插件的启用与显示状态。隐藏只隐藏入口，不停用；停用和移除停止观察器，保留计时记录及待补记数据。活动分段需先结束，再停用/更新/移除。

`DotLog.tomatodo.scan()`、`.import()`、`.compare()`、`.analyze()` 均需 `tomatodo:history`。宿主逐次检查安装、启用和授权，不开放任意文件路径、SQL或网络。导入仍遵循原有重叠检查和实时来源去重。旧直接历史IPC和旧番茄内置插件不再注册；既有历史来源设置保留给受控接口使用。

页面可额外声明 `tabs: [{id,label,action}]` 和 `activeTab`，最多5个标签。点击标签仍进入 `onAction`，由插件决定展示内容。按钮块支持 `disabled: boolean`。标签、字段和状态文字均由宿主按纯文本渲染，不解释为HTML。

备注草稿以插件ID、字段ID和页面分组跟踪；切换标签不清除草稿，真正移出补记队列后才清理对应草稿。联动状态应按 `status.enabled && status.context.connected` 显示已连接；状态读取失败不可继续显示旧的已连接状态。
