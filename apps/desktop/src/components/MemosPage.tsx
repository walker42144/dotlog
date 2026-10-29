import { useCallback, useEffect, useState } from "react";
import { ipc, IS_TAURI, friendlyMessage, toIpcError, type Memo, type MemoPage } from "../lib/ipc";
import "../styles/plugin-manager.css";
import { MemoContent, formatMemoTime } from "./MemoContent";

export function MemosPage({ onGoIntegrations }: { onGoIntegrations: () => void }) {
  const [page, setPage] = useState<MemoPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState("");
  const [visibility, setVisibility] = useState("PRIVATE");
  const [editing, setEditing] = useState<string | undefined>();
  const [deleting, setDeleting] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [hydratedKey, setHydratedKey] = useState<string | null>(null);
  const draftKey = page ? `dotlog.memo-draft:${page.instanceUrl}:${page.userId}` : null;
  useEffect(() => {
    if (!draftKey) return;
    try {
      const saved = JSON.parse(localStorage.getItem(draftKey) ?? "null") as { content?: string; visibility?: string; name?: string } | null;
      if (saved?.content) { setDraft(saved.content); setVisibility(saved.visibility ?? "PRIVATE"); setEditing(saved.name); }
    } catch { /* A damaged draft must not prevent browsing. */ }
    setHydratedKey(draftKey);
  }, [draftKey]);
  useEffect(() => {
    if (!draftKey || hydratedKey !== draftKey) return;
    try {
      if (draft) localStorage.setItem(draftKey, JSON.stringify({ content: draft, visibility, name: editing }));
      else localStorage.removeItem(draftKey);
    } catch { /* Storage limits do not block explicit remote saving. */ }
  }, [draftKey, hydratedKey, draft, visibility, editing]);
  const load = useCallback(async (token?: string) => {
    setLoading(true); setError(null);
    try {
      const next = await ipc.memosBrowse(token);
      setPage((old) => token && old ? { ...next, memos: [...old.memos, ...next.memos.filter((m) => !old.memos.some((o) => o.name === m.name))] } : next);
    } catch (e) { setError(friendlyMessage(toIpcError(e))); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const save = async () => {
    setBusy(true); setError(null);
    try {
      await ipc.memosSave(draft, visibility, editing);
      setDraft(""); setEditing(undefined); setVisibility("PRIVATE");
      await load();
    } catch (e) { setError(friendlyMessage(toIpcError(e))); }
    finally { setBusy(false); }
  };
  const remove = async (name: string) => {
    setBusy(true); setError(null);
    try { await ipc.memosDelete(name); setDeleting(null); await load(); }
    catch (e) { setError(friendlyMessage(toIpcError(e))); }
    finally { setBusy(false); }
  };
  const edit = (memo: Memo) => { setDraft(memo.content); setEditing(memo.name); setVisibility(memo.visibility); document.querySelector(".app-main")?.scrollTo({ top: 0 }); };
  return <div className="settings-page memos-client">
    <header className="page-head"><h1>Memos</h1><p>{page ? `${page.user} · 已通过插件 Access Token 连接` : "使用 Memos 插件配置自动连接，无需重复登录"}</p></header>
    <div className="plugin-manager-actions"><button className="ghost-btn" disabled={loading || busy} onClick={() => void load()}>刷新</button><button className="ghost-btn" onClick={onGoIntegrations}>插件配置</button>{!IS_TAURI && <span className="badge">浏览器预览 · 模拟数据</span>}</div>
    {error && <p role="alert" className="integration-card__error">{error}</p>}
    {page && <>
      <form className="settings-group" onSubmit={(e) => { e.preventDefault(); void save(); }}>
        <h2>{editing ? "编辑 Memo" : "写一条 Memo"}</h2><p className="modal__hint">未发布的内容自动保存为本机草稿，按实例和账号隔离。</p>
        <textarea aria-label="Memo 正文" className="field__input field__input--area" rows={5} maxLength={20000} value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="记录此刻的想法…" required disabled={busy} />
        <div className="plugin-manager-actions"><select aria-label="Memo 可见性" className="field__input setting-control" value={visibility} onChange={(e) => setVisibility(e.target.value)} disabled={busy}><option value="PRIVATE">私有</option><option value="PROTECTED">登录用户可见</option><option value="PUBLIC">公开</option></select><button className="primary-btn primary-btn--sm" disabled={busy || !draft.trim()}>{busy ? "处理中…" : editing ? "保存修改" : "发布到 Memos"}</button>{editing && <button type="button" className="ghost-btn" disabled={busy} onClick={() => { setEditing(undefined); setDraft(""); }}>取消编辑</button>}</div>
      </form>
      <input aria-label="搜索已加载的 Memos" className="field__input" type="search" value={search} onChange={(e) => setSearch(e.target.value)} placeholder="搜索已加载的记录" />
      {page.memos.filter((m) => m.content.toLowerCase().includes(search.toLowerCase())).map((memo) => <article className="integration-card memo-entry" key={memo.name}>
        <div className="plugin-manager-actions"><span className="modal__hint">{formatMemoTime(memo.createTime)} · {{ PRIVATE: "私有", PROTECTED: "登录用户可见", PUBLIC: "公开" }[memo.visibility] ?? "未知可见性"}</span><button className="ghost-btn" disabled={busy} onClick={() => edit(memo)}>编辑</button><button className="ghost-btn" disabled={busy} onClick={() => setDeleting(memo.name)}>删除</button></div>
        <MemoContent content={memo.content} />
        {deleting === memo.name && <div className="plugin-manager-actions" role="alert"><span>确定删除远端这条 Memo？</span><button className="danger-btn" disabled={busy} onClick={() => void remove(memo.name)}>确认删除</button><button className="ghost-btn" disabled={busy} onClick={() => setDeleting(null)}>取消</button></div>}
      </article>)}
      {!page.memos.length && !loading && <p className="panel__empty">还没有记录，可以在上方写下第一条。</p>}
      {page.nextPageToken && <button className="ghost-btn" disabled={loading || busy} onClick={() => void load(page.nextPageToken ?? undefined)}>加载更多</button>}
    </>}
    {loading && <p role="status">正在连接 Memos…</p>}
  </div>;
}
