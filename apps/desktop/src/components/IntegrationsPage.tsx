import { LocalPluginManager } from "./LocalPluginManager";
import { pluginsChanged } from "../lib/plugins";
import { useCallback, useEffect, useState } from "react";
import "../styles/plugin-manager.css";
import { listen } from "@tauri-apps/api/event";
import {
  friendlyMessage,
  ipc,
  IS_TAURI,
  toIpcError,
  type IntegrationCapability,
  type IntegrationView,
  type OutboxState,
  type OutboxView,
  type SettingsMap
} from "../lib/ipc";

const integrationName = (card: IntegrationView) => card.id === "tomatodo" ? "历史导入与对比" : card.name;

/** 同步状态事件（ipc-contract §4）。 */
const INTEGRATION_EVENT = "integration://status";

const CAPABILITY_LABEL: Record<IntegrationCapability, string> = {
  "segments:read": "读时间段",
  "tags:write": "写标签",
  network: "联网",
  "clipboard:read": "读剪贴板",
  "ui:panel": "界面面板",
  "secret:store": "系统钥匙串"
};

const OUTBOX_STATE_LABEL: Record<OutboxState, string> = {
  pending: "待发送",
  inflight: "发送中",
  done: "已同步",
  failed: "失败"
};

const PRIVACY_LABEL: Record<string, string> = {
  off: "不联网",
  title_only: "标题、时间与分类标签",
  with_note: "含备注全文"
};

const OUTBOX_FILTERS: Array<{ key: "all" | OutboxState; label: string }> = [
  { key: "all", label: "全部" },
  { key: "pending", label: "待发送" },
  { key: "failed", label: "失败" },
  { key: "done", label: "已同步" }
];

const pad = (value: number) => String(value).padStart(2, "0");

function formatClock(epochMs: number): string {
  const date = new Date(epochMs);
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function formatOutboxTime(epochMs: number): string {
  const date = new Date(epochMs);
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(
    date.getMinutes()
  )}`;
}

const readText = (settings: SettingsMap, key: string): string => {
  const value = settings[key];
  return typeof value === "string" ? value : "";
};

interface IntegrationsPageProps {
  onNotify: (message: string, tone?: "info" | "error") => void;
  onDataChanged: () => void;
  focusPlugin?: string;
  embedded?: boolean;
}


export function IntegrationsPage({ onNotify, focusPlugin, embedded = false }: IntegrationsPageProps) {
  const [cards, setCards] = useState<IntegrationView[] | null>(null);
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const [settingsById, setSettingsById] = useState<Record<string, SettingsMap>>({});
  const [memosUrl, setMemosUrl] = useState("");
  const [memosToken, setMemosToken] = useState("");
  const [memosAutoPublish, setMemosAutoPublish] = useState("off");
  const [memosVisibility, setMemosVisibility] = useState("private");
  const [memosIncludeNote, setMemosIncludeNote] = useState(false);
  const [memosTagRules, setMemosTagRules] = useState("");
  const [aiProvider, setAiProvider] = useState("ollama");
  const [aiBaseUrl, setAiBaseUrl] = useState("");
  const [aiModel, setAiModel] = useState("");
  const [aiKey, setAiKey] = useState("");
  const [aiModels, setAiModels] = useState<string[]>([]);
  const [aiPrivacy, setAiPrivacy] = useState("title_only");

  const [outbox, setOutbox] = useState<OutboxView[]>([]);
  const [outboxFilter, setOutboxFilter] = useState<"all" | OutboxState>("all");

  const [confirmEnable, setConfirmEnable] = useState<IntegrationView | null>(null);

  // 番茄Todo

  const loadAll = useCallback(async () => {
    try {
      const [list, queue] = await Promise.all([ipc.integrationList(), ipc.outboxList()]);
      setCards(list);
      setOutbox(queue);
      const entries = await Promise.all(
        list.map(async (card) => [card.id, await ipc.settingsGet(`plugin:${card.id}`)] as const)
      );
      const next: Record<string, SettingsMap> = {};
      for (const [id, value] of entries) next[id] = value;
      setSettingsById(next);
      setMemosUrl(readText(next.memos ?? {}, "instanceUrl"));
      setMemosAutoPublish(readText(next.memos ?? {}, "autoPublish") || "off");
      setMemosVisibility(readText(next.memos ?? {}, "visibility") || "private");
      setMemosIncludeNote(
        (next.memos?.includeNote as boolean | undefined) ?? false
      );
      setMemosTagRules(readText(next.memos ?? {}, "tagRules"));
      setAiProvider(readText(next.ai ?? {}, "provider") || "ollama");
      setAiBaseUrl(readText(next.ai ?? {}, "baseUrl"));
      setAiModel(readText(next.ai ?? {}, "model"));
      setAiPrivacy(readText(next.ai ?? {}, "privacyMode") || "title_only");
      setLoadError(null);
    } catch (reason) {
      setLoadError(friendlyMessage(toIpcError(reason)));
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  // host 线程的同步状态事件（§4）：到达即合并对应卡片。
  useEffect(() => {
    if (!IS_TAURI) return;
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void listen<IntegrationView>(INTEGRATION_EVENT, (event) => {
      setCards((current) =>
        current?.map((card) => (card.id === event.payload.id ? event.payload : card)) ?? current
      );
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const toggleEnabled = useCallback(
    async (card: IntegrationView, next: boolean) => {
      // 首次启用联网插件：先确认将发送哪些字段（ui-spec §4.3）。
      if (next && card.capabilities.includes("network")) {
        setConfirmEnable(card);
        return;
      }
      setBusy(`${card.id}-enabled`);
      try {
        const view = await ipc.integrationSetEnabled(card.id, next);
      pluginsChanged();
        setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
        onNotify(next ? `已启用 ${integrationName(view)}` : `已停用 ${integrationName(view)}`);
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [onNotify]
  );

  const confirmNetworkEnable = useCallback(async () => {
    const card = confirmEnable;
    if (!card) return;
    setConfirmEnable(null);
    setBusy(`${card.id}-enabled`);
    try {
      const view = await ipc.integrationSetEnabled(card.id, true);
      pluginsChanged();
      setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
      onNotify(`已启用 ${integrationName(view)}`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  }, [confirmEnable, onNotify]);

  const saveConfig = useCallback(
    async (id: "memos" | "ai") => {
      setBusy(`${id}-config`);
      try {
        if (id === "memos") {
          const view = await ipc.integrationConfigSet(id, {
            instanceUrl: memosUrl.trim(),
            autoPublish: memosAutoPublish,
            visibility: memosVisibility,
            includeNote: memosIncludeNote,
            tagRules: memosTagRules.trim()
          });
          if (memosToken.trim() !== "") {
            await ipc.integrationSecretSet(id, memosToken.trim());
          }
          setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
          setMemosToken("");
        } else {
          const view = await ipc.integrationConfigSet(id, {
            provider: aiProvider,
            baseUrl: aiBaseUrl.trim(),
            model: aiModel.trim(),
            privacyMode: aiPrivacy
          });
          if (aiKey.trim() !== "") {
            await ipc.integrationSecretSet(id, aiKey.trim());
          }
          setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
          setAiKey("");
        }
        pluginsChanged();
        onNotify("配置已保存");
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [aiBaseUrl, aiKey, aiModel, aiPrivacy, aiProvider, memosAutoPublish, memosIncludeNote, memosTagRules, memosToken, memosUrl, memosVisibility, onNotify]
  );

  const loadAiModels = async () => {
    setBusy("ai-models");
    try { const models=await ipc.aiModels(); setAiModels(models); if(!models.length)onNotify("服务未列出模型，可手动填写模型 ID"); }
    catch(e){onNotify(friendlyMessage(toIpcError(e)),"error");} finally {setBusy(null);}
  };

  const testConnection = useCallback(
    async (id: string) => {
      setBusy(`${id}-test`);
      try {
        const result = await ipc.integrationTest(id);
        onNotify(result.ok ? `连接成功：${result.message}` : result.message, result.ok ? "info" : "error");
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [onNotify]
  );

  const retryOutbox = useCallback(
    async (id: string) => {
      setBusy(`outbox-${id}`);
      try {
        const row = await ipc.outboxRetry(id);
        setOutbox((current) =>
          current.map((item) => (item.id === row.id ? row : item))
        );
        onNotify(row.state === "done" ? "已重新发送" : "已加入重试队列");
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [onNotify]
  );

  if (cards === null) {
    return (
      <div className={embedded ? "integration-embedded" : "settings-page"}>
        {!embedded && <header className="page-head">
          <h1>集成</h1>
          <p>{loadError ?? "读取中…"}</p>
        </header>}
      </div>
    );
  }

  const installed = cards.filter((card) => card.id !== "tomatodo" && card.installed !== false && (!focusPlugin || card.id === focusPlugin));
  const available = cards.filter((card) => card.id !== "tomatodo" && card.installed === false);
  const changeInstallation = async (card: IntegrationView, value: boolean) => {
    setBusy(`${card.id}-installed`);
    try {
      const view = await ipc.integrationSetInstalled(card.id, value);
      pluginsChanged();
      setCards((current) => current?.map((item) => item.id === view.id ? view : item) ?? current);
      onNotify(value ? `已添加 ${integrationName(card)}，启用后开始工作` : `已移除 ${integrationName(card)}，配置和本地记录已保留`);
    } catch (e) { onNotify(friendlyMessage(toIpcError(e)), "error"); }
    finally { setBusy(null); }
  };
  const visibleOutbox = outboxFilter === "all" ? outbox : outbox.filter((row) => row.state === outboxFilter);
  const memosCard = cards.find((card) => card.id === "memos");
  const aiCard = cards.find((card) => card.id === "ai");
  const memosSettings = settingsById.memos ?? {};
  const memosVersion = readText(memosSettings, "apiVersion");
  const memosSynced = memosCard?.lastSyncAt != null ? formatClock(memosCard.lastSyncAt) : null;
  const aiPrivacyText = PRIVACY_LABEL[aiPrivacy] ?? aiPrivacy;

  return (
    <div className={embedded ? "integration-embedded" : "settings-page"}>
      {!embedded && <header className="page-head">
        <h1>{focusPlugin ? cards.find(p=>p.id===focusPlugin)?.name : "插件管理"}</h1>
        <p>核心功能无需任何集成 · 未启用的插件不产生任何联网请求</p>
      </header>}

      {!focusPlugin && <LocalPluginManager />}
      <section className="settings-group">
        <div className="plugin-manager-actions"><h2>{embedded ? (focusPlugin === "ai" ? "连接设置" : "历史数据") : focusPlugin ? "插件设置" : "已添加的插件"}</h2><button className="primary-btn primary-btn--sm" type="button" hidden={!!focusPlugin} onClick={() => setCatalogOpen((v) => !v)}>{catalogOpen ? "收起插件目录" : "＋ 添加插件"}</button></div>
        {catalogOpen && <div className="plugin-catalog">
          <h3>插件目录</h3><p>选择需要的功能，添加后单独配置与启用。移除会停止插件，保留配置便于重新添加。</p>
          {available.length === 0 ? <p>目录中的插件均已添加。</p> : available.map((card) => <div key={card.id} className="setting-row"><div><strong>{integrationName(card)}</strong><p>{card.capabilities.map((cap) => CAPABILITY_LABEL[cap]).join(" · ")}</p></div><button type="button" className="ghost-btn" disabled={busy !== null} onClick={() => void changeInstallation(card, true)}>添加</button></div>)}
          <p className="modal__hint">内置插件在此添加；本地扩展请使用上方“从本地安装”。MCP服务暂不支持。</p>
        </div>}
        {installed.length === 0 ? (
          <p className="panel__empty">还没有可用插件，所有数据都保存在本机</p>
        ) : (
          installed.map((card) => (
            <div key={card.id} className="integration-card">
              <div className="integration-card__head">
                <div className="integration-card__title">
                  <strong>{integrationName(card)}</strong>
                  <span className="integration-card__version">v{card.version}</span>
                  <span
                    className={`outbox-badge outbox-badge--${card.enabled ? (card.configured ? "done" : "pending") : "off"}`}
                  >
                    {card.enabled ? (card.configured ? "已配置" : "待配置") : "未启用"}
                  </span>
                </div>
                <div className="plugin-manager-actions">
                <button type="button" className="ghost-btn" disabled={busy !== null} onClick={() => void changeInstallation(card, false)}>移除</button>
                <label className="switch">
                  <input
                    type="checkbox"
                    checked={card.enabled}
                    disabled={busy !== null}
                    aria-label={`启用 ${integrationName(card)}`}
                    onChange={(event) => void toggleEnabled(card, event.target.checked)}
                  />
                  <span className="switch__track" aria-hidden="true" />
                </label>
                </div>
              </div>

              <p className="integration-card__desc">
                {card.id === "memos"
                  ? "记录关闭后延迟发布最新内容，允许补写与撤销。侧边 Memos 共用此处的实例地址与 Access Token，无需再次登录。"
                  : card.id === "ai"
                    ? "用本地或 OpenAI 兼容服务整理标题与分类，建议需逐条确认。"
                    : card.id === "tomatodo"
                      ? "读取本机番茄Todo 数据，把专注记录导入 DotLog 并对比分析两者差异。"
                      : ""}
              </p>

              <div className="integration-card__caps">
                {card.capabilities.map((cap) => (
                  <span key={cap} className="cap-chip">
                    {CAPABILITY_LABEL[cap] ?? cap}
                  </span>
                ))}
              </div>

              {card.lastError && <p className="integration-card__error">最近错误：{card.lastError}</p>}
              {!card.lastError && card.lastSyncAt != null && (
                <p className="integration-card__meta">最近同步：{formatClock(card.lastSyncAt)}</p>
              )}

              {card.enabled && card.id === "memos" && (
                <div className="integration-card__form">
                  <label className="integration-field">
                    <span>实例地址</span>
                    <input
                      className="field__input"
                      type="url"
                      value={memosUrl}
                      placeholder="https://memos.example.com"
                      onChange={(event) => setMemosUrl(event.target.value)}
                    />
                  </label>
                  <label className="integration-field">
                    <span>访问令牌</span>
                    <input
                      className="field__input"
                      type="password"
                      value={memosToken}
                      placeholder="已保存则留空即可"
                      autoComplete="new-password"
                      onChange={(event) => setMemosToken(event.target.value)}
                    />
                    <small>存入系统钥匙串，不会写入数据库或日志</small>
                  </label>
                  <div className="integration-field-row">
                    <label className="integration-field">
                      <span>自动发布</span>
                      <select
                        className="field__input"
                        value={memosAutoPublish}
                        onChange={(event) => setMemosAutoPublish(event.target.value)}
                      >
                        <option value="off">不自动发布</option>
                        <option value="on_close">段关闭时自动发布</option>
                        <option value="tag_rule">按标签规则发布</option>
                      </select>
                    </label>
                    <label className="integration-field">
                      <span>可见性</span>
                      <select
                        className="field__input"
                        value={memosVisibility}
                        onChange={(event) => setMemosVisibility(event.target.value)}
                      >
                        <option value="private">私有</option>
                        <option value="protected">仅登录可见</option>
                        <option value="public">公开</option>
                      </select>
                    </label>
                  </div>
                  {memosAutoPublish === "tag_rule" && (
                    <label className="integration-field">
                      <span>标签规则</span>
                      <input
                        className="field__input"
                        type="text"
                        value={memosTagRules}
                        placeholder="逗号分隔，如 学习,工作；留空则有标签即发布"
                        onChange={(event) => setMemosTagRules(event.target.value)}
                      />
                    </label>
                  )}
                  <div className="setting-row" style={{ borderTop: 0, paddingBottom: 0 }}>
                    <div className="setting-row__text">
                      <label>包含备注</label>
                      <p>发布包含标题、分类、标签、专注状态、有效时长和暂停合计的完整记录；此开关决定是否附带备注全文。</p>
                    </div>
                    <label className="switch">
                      <input
                        type="checkbox"
                        checked={memosIncludeNote}
                        onChange={(event) => setMemosIncludeNote(event.target.checked)}
                        aria-label="发布时包含备注"
                      />
                      <span className="switch__track" aria-hidden="true" />
                    </label>
                  </div>
                  <div className="integration-actions">
                    <button
                      type="button"
                      className="ghost-btn"
                      disabled={busy !== null}
                      onClick={() => void testConnection("memos")}
                    >
                      测试连接
                    </button>
                    <button
                      type="button"
                      className="primary-btn primary-btn--sm"
                      disabled={busy !== null}
                      onClick={() => void saveConfig("memos")}
                    >
                      保存
                    </button>
                    {(memosVersion || memosSynced) && (
                      <span className="integration-actions__meta">
                        {memosVersion && `API ${memosVersion}`}
                        {memosVersion && memosSynced && " · "}
                        {memosSynced && `上次同步 ${memosSynced}`}
                      </span>
                    )}
                  </div>
                </div>
              )}

              {card.enabled && card.id === "ai" && (
                <div className="integration-card__form">
                  <div className="integration-field-row">
                    <label className="integration-field">
                      <span>提供方</span>
                      <select
                        className="field__input"
                        value={aiProvider}
                        onChange={(event) => setAiProvider(event.target.value)}
                      >
                        <option value="ollama">Ollama（本机）</option>
                        <option value="openai">OpenAI</option>
                        <option value="custom">自定义</option>
                      </select>
                    </label>
                    <label className="integration-field">
                      <span>隐私模式</span>
                      <select
                        className="field__input"
                        value={aiPrivacy}
                        onChange={(event) => setAiPrivacy(event.target.value)}
                      >
                        <option value="title_only">标题、时间与分类标签</option>
                        <option value="with_note">含备注全文</option>
                        <option value="off">不联网</option>
                      </select>
                    </label>
                  </div>
                  <label className="integration-field">
                    <span>服务地址</span>
                    <input
                      className="field__input"
                      type="url"
                      value={aiBaseUrl}
                      placeholder="http://localhost:11434/v1"
                      onChange={(event) => setAiBaseUrl(event.target.value)}
                    />
                  </label>
                  <label className="integration-field">
                    <span>模型</span>
                    <input
                      className="field__input"
                      type="text"
                      value={aiModel}
                      placeholder="qwen2.5:7b"
                      onChange={(event) => setAiModel(event.target.value)}
                    />
                  </label>
                  <label className="integration-field">
                    <span>API 密钥</span>
                    <input
                      className="field__input"
                      type="password"
                      value={aiKey}
                      placeholder="本地 Ollama 可留空"
                      autoComplete="new-password"
                      onChange={(event) => setAiKey(event.target.value)}
                    />
                    <small>存入系统钥匙串，不会写入数据库或日志</small>
                  </label>
                  <div className="integration-actions">
                    <button className="ghost-btn" type="button" disabled={busy!==null||aiPrivacy==="off"} onClick={()=>void loadAiModels()}>{busy==="ai-models"?"读取中…":"读取可用模型"}</button>
                    {aiModels.length>0&&<select aria-label="可用模型" className="field__input ai-model-select" value={aiModels.includes(aiModel)?aiModel:""} onChange={e=>setAiModel(e.target.value)}><option value="" disabled>选择模型后点击保存</option>{aiModels.map(model=><option key={model} value={model}>{model}</option>)}</select>}
                  </div>
                  <p className="modal__hint">先保存服务地址和密钥，再读取模型列表或测试连接；不支持列出模型的服务可手动填写模型 ID。</p>
                  <div className="integration-actions">
                    <button
                      type="button"
                      className="ghost-btn"
                      disabled={busy !== null || aiPrivacy === "off"}
                      onClick={() => void testConnection("ai")}
                    >
                      测试连接
                    </button>
                    <button
                      type="button"
                      className="primary-btn primary-btn--sm"
                      disabled={busy !== null}
                      onClick={() => void saveConfig("ai")}
                    >
                      保存
                    </button>
                    <span className="integration-actions__meta">隐私模式：{aiPrivacyText}</span>
                  </div>
                </div>
              )}


            </div>
          ))
        )}
      </section>

      {!focusPlugin && aiCard?.enabled && <section className="settings-group ai-section"><h2>AI 工作台</h2><p>按日期整理记录，编辑后逐条采纳；生成日/周复盘，或围绕所选时间提问。</p><button className="primary-btn primary-btn--sm" onClick={()=>window.dispatchEvent(new CustomEvent("dotlog:open-plugin",{detail:"ai"}))}>打开 AI 工作台</button></section>}

      {!focusPlugin && <section className="settings-group">
        <h2>发布队列</h2>
        <div className="outbox-filters" role="group" aria-label="按状态筛选">
          {OUTBOX_FILTERS.map((filter) => (
            <button
              key={filter.key}
              type="button"
              className={`outbox-filter${outboxFilter === filter.key ? " outbox-filter--active" : ""}`}
              aria-pressed={outboxFilter === filter.key}
              onClick={() => setOutboxFilter(filter.key)}
            >
              {filter.label}
            </button>
          ))}
        </div>

        {visibleOutbox.length === 0 ? (
          <p className="panel__empty">队列为空 · 记录关闭后自动进入发布队列</p>
        ) : (
          <div className="outbox-table">
            <div className="outbox-table__head">
              <span>时间</span>
              <span>标题</span>
              <span>尝试</span>
              <span>状态</span>
              <span />
            </div>
            {visibleOutbox.map((row) => (
              <div key={row.id} className={`outbox-table__row outbox-table__row--${row.state}`}>
                <span className="outbox-table__time">{formatOutboxTime(row.updatedAtMs)}</span>
                <span title={row.lastError ?? undefined}>
                  {row.title ?? row.eventType}
                  {row.lastError && <em className="outbox-table__error">{row.lastError}</em>}
                </span>
                <span className="outbox-table__num">{row.attempts}</span>
                <span>
                  <span className={`outbox-badge outbox-badge--${row.state}`}>
                    {OUTBOX_STATE_LABEL[row.state]}
                  </span>
                </span>
                <span className="outbox-table__actions">
                  {row.state === "failed" && (
                    <button
                      type="button"
                      className="ghost-btn ghost-btn--sm"
                      disabled={busy !== null}
                      onClick={() => void retryOutbox(row.id)}
                    >
                      重试
                    </button>
                  )}
                </span>
              </div>
            ))}
          </div>
        )}
      </section>}

      {!focusPlugin && <section className="settings-group">
        <h2>隐私</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>联网范围</label>
            <p>
              Memos 浏览与编辑按你的操作请求实例，自动发布受配置控制。请求仅发往配置的服务，跨站重定向被拒绝；令牌只存系统钥匙串。AI 建议需手动触发并确认后写回。
            </p>
          </div>
          <span className="badge">{cards.some((card) => card.enabled) ? "部分启用" : "未启用"}</span>
        </div>
      </section>}

      {confirmEnable && (
        <div className="modal-backdrop" onClick={() => setConfirmEnable(null)}>
          <div
            className="modal"
            role="dialog"
            aria-label="启用联网插件"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="modal__head">
              <strong>启用 {confirmEnable.name}？</strong>
            </div>
            <p className="modal__hint">
              {confirmEnable.id === "memos" ? "允许侧边 Memos 使用令牌读取和编辑远端记录。自动发布默认关闭，开启后发送标题、分类、标签、专注状态、时间与暂停汇总，备注由包含备注开关控制。" : "AI 仅在手动获取建议时发送隐私模式允许的字段；建议须确认后写入。"} 密钥存入系统钥匙串，本地时间记录保留在本机。
            </p>
            <div className="modal__foot">
              <div className="modal__actions">
                <button type="button" className="ghost-btn" onClick={() => setConfirmEnable(null)}>
                  取消
                </button>
                <button
                  type="button"
                  className="primary-btn primary-btn--sm"
                  disabled={busy !== null}
                  onClick={() => void confirmNetworkEnable()}
                >
                  确认启用
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
