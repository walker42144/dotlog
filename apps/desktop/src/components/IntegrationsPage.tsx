import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  friendlyMessage,
  ipc,
  IS_TAURI,
  toIpcError,
  type AiSuggestion,
  type IntegrationCapability,
  type IntegrationView,
  type OutboxState,
  type OutboxView,
  type SegmentView,
  type SettingsMap,
  type TomatodoAnalysis,
  type TomatodoCompareResult,
  type TomatodoImportResult,
  type TomatodoScanResult
} from "../lib/ipc";

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
  title_only: "仅标题与时间",
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
}

interface SuggestionRow {
  suggestion: AiSuggestion;
  segment: SegmentView;
}

export function IntegrationsPage({ onNotify, onDataChanged }: IntegrationsPageProps) {
  const [cards, setCards] = useState<IntegrationView[] | null>(null);
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
  const [aiPrivacy, setAiPrivacy] = useState("title_only");

  const [outbox, setOutbox] = useState<OutboxView[]>([]);
  const [outboxFilter, setOutboxFilter] = useState<"all" | OutboxState>("all");

  const [confirmEnable, setConfirmEnable] = useState<IntegrationView | null>(null);
  const [suggestions, setSuggestions] = useState<SuggestionRow[]>([]);
  const [aiBusy, setAiBusy] = useState(false);

  // 番茄Todo
  const [tomatodoScan, setTomatodoScan] = useState<TomatodoScanResult | null>(null);
  const [tomatodoImport, setTomatodoImport] = useState<TomatodoImportResult | null>(null);
  const [tomatodoCompare, setTomatodoCompare] = useState<TomatodoCompareResult | null>(null);
  const [tomatodoAnalysis, setTomatodoAnalysis] = useState<TomatodoAnalysis[] | null>(null);
  const [tomatodoBusy, setTomatodoBusy] = useState<string | null>(null);

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
        setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
        onNotify(next ? `已启用 ${view.name}` : `已停用 ${view.name}`);
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
      setCards((current) => current?.map((item) => (item.id === view.id ? view : item)) ?? current);
      onNotify(`已启用 ${view.name}`);
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
        onNotify("配置已保存");
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [aiBaseUrl, aiKey, aiModel, aiPrivacy, aiProvider, memosAutoPublish, memosIncludeNote, memosTagRules, memosToken, memosUrl, memosVisibility, onNotify]
  );

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

  const loadSuggestions = useCallback(async () => {
    setAiBusy(true);
    try {
      const [list, day] = await Promise.all([ipc.aiSuggest(), ipc.dayToday()]);
      const byId = new Map(day.segments.map((segment) => [segment.id, segment]));
      setSuggestions(
        list
          .map((suggestion) => {
            const segment = byId.get(suggestion.segmentId);
            return segment ? { suggestion, segment } : null;
          })
          .filter((row): row is SuggestionRow => row !== null)
      );
      if (list.length === 0) onNotify("今天的记录都整理好了");
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setAiBusy(false);
    }
  }, [onNotify]);

  const applySuggestion = useCallback(
    async (row: SuggestionRow) => {
      const { suggestion, segment } = row;
      setBusy(`ai-${suggestion.segmentId}`);
      try {
        await ipc.aiApply({
          segmentId: suggestion.segmentId,
          revision: segment.revision,
          title: suggestion.title ?? undefined,
          categoryId: suggestion.categoryId ?? undefined,
          tags: suggestion.tags
        });
        setSuggestions((current) => current.filter((item) => item.suggestion.segmentId !== suggestion.segmentId));
        onNotify("建议已应用");
        onDataChanged();
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [onDataChanged, onNotify]
  );

  // ---------- 番茄Todo 操作 ----------

  const scanTomatodo = useCallback(async () => {
    setTomatodoBusy("scan");
    try {
      const result = await ipc.tomatodoScan();
      setTomatodoScan(result);
      onNotify(`扫描完成：${result.completedRecords} 条专注记录，共 ${result.totalMinutes} 分钟`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setTomatodoBusy(null);
    }
  }, [onNotify]);

  const importTomatodo = useCallback(async () => {
    setTomatodoBusy("import");
    try {
      const result = await ipc.tomatodoImport();
      setTomatodoImport(result);
      const parts = [
        `导入 ${result.imported} 条`,
        result.skippedExisting > 0 ? `跳过 ${result.skippedExisting} 条已导入` : "",
        result.skippedIncomplete > 0 ? `忽略 ${result.skippedIncomplete} 条未完成` : "",
        result.conflicts > 0 ? `冲突 ${result.conflicts} 条` : ""
      ].filter(Boolean);
      onNotify(parts.join("，"));
      onDataChanged();
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setTomatodoBusy(null);
    }
  }, [onDataChanged, onNotify]);

  const compareTomatodo = useCallback(async () => {
    setTomatodoBusy("compare");
    try {
      const result = await ipc.tomatodoCompare();
      setTomatodoCompare(result);
      onNotify(
        `对比完成：匹配 ${result.summary.matchedPairs} 对，DotLog 缺 ${result.summary.missingInDotlog} 条，多 ${result.summary.extraInDotlog} 条`
      );
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setTomatodoBusy(null);
    }
  }, [onNotify]);

  const analyzeTomatodo = useCallback(async () => {
    setTomatodoBusy("analyze");
    try {
      const result = await ipc.tomatodoAnalyze();
      setTomatodoAnalysis(result);
      onNotify(`智能分析完成：${result.length} 条建议`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setTomatodoBusy(null);
    }
  }, [onNotify]);

  if (cards === null) {
    return (
      <div className="settings-page">
        <header className="page-head">
          <h1>集成</h1>
          <p>{loadError ?? "读取中…"}</p>
        </header>
      </div>
    );
  }

  const visibleOutbox = outboxFilter === "all" ? outbox : outbox.filter((row) => row.state === outboxFilter);
  const memosCard = cards.find((card) => card.id === "memos");
  const aiCard = cards.find((card) => card.id === "ai");
  const memosSettings = settingsById.memos ?? {};
  const memosVersion = readText(memosSettings, "apiVersion");
  const memosSynced = memosCard?.lastSyncAt != null ? formatClock(memosCard.lastSyncAt) : null;
  const aiPrivacyText = PRIVACY_LABEL[aiPrivacy] ?? aiPrivacy;

  return (
    <div className="settings-page">
      <header className="page-head">
        <h1>集成</h1>
        <p>核心功能无需任何集成 · 未启用的插件不产生任何联网请求</p>
      </header>

      <section className="settings-group">
        <h2>插件</h2>
        {cards.length === 0 ? (
          <p className="panel__empty">还没有可用插件，所有数据都保存在本机</p>
        ) : (
          cards.map((card) => (
            <div key={card.id} className="integration-card">
              <div className="integration-card__head">
                <div className="integration-card__title">
                  <strong>{card.name}</strong>
                  <span className="integration-card__version">v{card.version}</span>
                  <span
                    className={`outbox-badge outbox-badge--${card.enabled ? (card.configured ? "done" : "pending") : "off"}`}
                  >
                    {card.enabled ? (card.configured ? "已配置" : "待配置") : "未启用"}
                  </span>
                </div>
                <label className="switch">
                  <input
                    type="checkbox"
                    checked={card.enabled}
                    disabled={busy !== null}
                    aria-label={`启用 ${card.name}`}
                    onChange={(event) => void toggleEnabled(card, event.target.checked)}
                  />
                  <span className="switch__track" aria-hidden="true" />
                </label>
              </div>

              <p className="integration-card__desc">
                {card.id === "memos"
                  ? "记录关闭后自动发布为 Memos 日记，幂等重试，不打断打点。"
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
                      <p>默认只发布标题与时间，开启后附带备注全文</p>
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
                        <option value="title_only">仅标题与时间</option>
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

              {card.enabled && card.id === "tomatodo" && (
                <div className="integration-card__form">
                  <div className="integration-actions">
                    <button
                      type="button"
                      className="ghost-btn"
                      disabled={tomatodoBusy !== null}
                      onClick={() => void scanTomatodo()}
                    >
                      {tomatodoBusy === "scan" ? "扫描中…" : "扫描"}
                    </button>
                    <button
                      type="button"
                      className="primary-btn primary-btn--sm"
                      disabled={tomatodoBusy !== null}
                      onClick={() => void importTomatodo()}
                    >
                      {tomatodoBusy === "import" ? "导入中…" : "导入"}
                    </button>
                    <button
                      type="button"
                      className="ghost-btn"
                      disabled={tomatodoBusy !== null}
                      onClick={() => void compareTomatodo()}
                    >
                      {tomatodoBusy === "compare" ? "对比中…" : "对比"}
                    </button>
                    <button
                      type="button"
                      className="ghost-btn"
                      disabled={tomatodoBusy !== null || !aiCard?.enabled}
                      onClick={() => void analyzeTomatodo()}
                      title={aiCard?.enabled ? "" : "需先启用 AI 助手"}
                    >
                      {tomatodoBusy === "analyze" ? "分析中…" : "智能分析"}
                    </button>
                  </div>

                  {tomatodoScan && (
                    <div className="tomatodo-panel">
                      <div className="tomatodo-stats">
                        <span>专注记录 {tomatodoScan.completedRecords}/{tomatodoScan.totalRecords} 条</span>
                        <span>共 {tomatodoScan.totalMinutes} 分钟</span>
                        <span>待办 {tomatodoScan.todoCount} 条</span>
                      </div>
                      <div className="tomatodo-preview">
                        {tomatodoScan.preview.map((item) => (
                          <div key={item.id} className="tomatodo-preview__row">
                            <span className="tomatodo-preview__name">{item.name}</span>
                            <span className="tomatodo-preview__time">
                              {item.startText}–{item.endText}
                            </span>
                            <span className="tomatodo-preview__dur">{item.durationMinutes}min</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  {tomatodoImport && (
                    <div className="tomatodo-import-result">
                      导入结果：成功 {tomatodoImport.imported} · 跳过 {tomatodoImport.skippedExisting} ·
                      忽略未完成 {tomatodoImport.skippedIncomplete} · 冲突 {tomatodoImport.conflicts}
                      {tomatodoImport.errors.length > 0 && (
                        <ul>
                          {tomatodoImport.errors.map((err, i) => (
                            <li key={i}>{err}</li>
                          ))}
                        </ul>
                      )}
                    </div>
                  )}

                  {tomatodoCompare && (
                    <div className="tomatodo-compare">
                      <div className="tomatodo-stats">
                        <span>番茄Todo {tomatodoCompare.summary.tomatodoMinutes}min</span>
                        <span>DotLog {tomatodoCompare.summary.dotlogMinutes}min</span>
                        <span>匹配 {tomatodoCompare.summary.matchedPairs} 对</span>
                        <span>DotLog 缺 {tomatodoCompare.summary.missingInDotlog}</span>
                        <span>DotLog 多 {tomatodoCompare.summary.extraInDotlog}</span>
                      </div>
                      <div className="tomatodo-compare__list">
                        {tomatodoCompare.pairs.map((pair, idx) => (
                          <div key={idx} className="tomatodo-compare__row">
                            <div className="tomatodo-compare__left">
                              {pair.tomatodo ? (
                                <>
                                  <strong>{pair.tomatodo.name}</strong>
                                  <span>{pair.tomatodo.durationMinutes}min · {pair.tomatodo.startText}</span>
                                </>
                              ) : (
                                <span className="tomatodo-compare__none">番茄Todo 无记录</span>
                              )}
                            </div>
                            <div className="tomatodo-compare__coverage">
                              覆盖 {pair.coveragePercent}%
                            </div>
                            <div className="tomatodo-compare__right">
                              {pair.dotlogSegments.length > 0 ? (
                                pair.dotlogSegments.map((seg) => (
                                  <span key={seg.id} className="tomatodo-compare__seg">
                                    {seg.title}
                                    {seg.categoryName && `（${seg.categoryName}）`}
                                  </span>
                                ))
                              ) : (
                                <span className="tomatodo-compare__none">DotLog 无记录</span>
                              )}
                            </div>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  {tomatodoAnalysis && (
                    <div className="tomatodo-analysis">
                      {tomatodoAnalysis.length === 0 ? (
                        <p className="panel__empty">暂无可分析的配对</p>
                      ) : (
                        <ul className="tomatodo-analysis__list">
                          {tomatodoAnalysis.map((item) => (
                            <li key={item.pairIndex} className="tomatodo-analysis__item">
                              <div className="tomatodo-analysis__subject">
                                <strong>{item.tomatodoSubject}</strong>
                                {item.dotlogSummary && <span>→ {item.dotlogSummary}</span>}
                              </div>
                              <p className="tomatodo-analysis__suggestion">{item.suggestion}</p>
                              <p className="tomatodo-analysis__reason">{item.reason}</p>
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          ))
        )}
      </section>

      {aiCard?.enabled && (
        <section className="settings-group">
          <h2>AI 整理建议</h2>
          <div className="setting-row setting-row--stack">
            <div className="setting-row__text">
              <label>整理今天的记录</label>
              <p>
                仅发送标题与起止时间（隐私模式：{aiPrivacyText}）；建议只在确认后写入记录。
              </p>
            </div>
            <div className="settings-actions">
              <button
                type="button"
                className="primary-btn primary-btn--sm"
                disabled={aiBusy || busy !== null}
                onClick={() => void loadSuggestions()}
              >
                {aiBusy ? "分析中…" : "获取建议"}
              </button>
            </div>
          </div>

          {suggestions.length > 0 && (
            <ul className="ai-suggest">
              {suggestions.map(({ suggestion, segment }) => (
                <li key={suggestion.segmentId} className="ai-suggest__item">
                  <div className="ai-suggest__time">
                    {formatClock(segment.startAt)}–{formatClock(segment.endAt ?? Date.now())}
                  </div>
                  <div className="ai-suggest__body">
                    <div className="ai-suggest__titles">
                      <span className="ai-suggest__old">{segment.title || "未命名"}</span>
                      {suggestion.title && suggestion.title !== segment.title && (
                        <>
                          <span aria-hidden="true">→</span>
                          <span className="ai-suggest__new">{suggestion.title}</span>
                        </>
                      )}
                    </div>
                    <div className="ai-suggest__tags">
                      {suggestion.tags.map((tag) => (
                        <span key={tag} className="tag-chip">
                          #{tag}
                        </span>
                      ))}
                      <span className="ai-suggest__reason">{suggestion.reason}</span>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="ghost-btn"
                    disabled={busy !== null}
                    onClick={() => void applySuggestion({ suggestion, segment })}
                  >
                    应用
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <section className="settings-group">
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
      </section>

      <section className="settings-group">
        <h2>隐私</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>联网范围</label>
            <p>
              只在记录关闭后入队发送；请求域名受插件清单白名单限制；密钥只存系统钥匙串。AI 建议需手动触发，确认后才写回记录。
            </p>
          </div>
          <span className="badge">{cards.some((card) => card.enabled) ? "部分启用" : "未启用"}</span>
        </div>
      </section>

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
              启用后，每次记录关闭时会向该服务发送：标题、起止时间、分类与标签
              {confirmEnable.id === "ai" ? "（AI 助手仅在手动获取建议时联网）" : ""}。密钥存入系统钥匙串，本地记录始终保留完整副本。
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
