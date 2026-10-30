import { validPluginView } from "../lib/plugin-view";
import { useEffect, useRef, useState } from "react";
import { IS_TAURI } from "../lib/ipc";
import { MemoContent } from "./MemoContent";
import { localPlugins, type LocalPlugin } from "../lib/plugins";
type Block = {
  type:
    "heading" | "text" | "markdown" | "stat" | "input" | "textarea" | "button";
  id?: string;
  text?: string;
  value?: string;
  label?: string;
  action?: string;
  disabled?: boolean;
  inputType?: "date" | "text";
};
type ViewTab = {id:string;label:string;action:string};
type View = { title?: string; tabs?: ViewTab[]; activeTab?:string; blocks: Block[] };
const BOOTSTRAP = `
const requests=new Map();let next=0;let action=()=>{};
const call=(method,params={})=>new Promise((resolve,reject)=>{
 const id=++next;const timer=setTimeout(()=>{requests.delete(id);reject(new Error('请求超时'));},method==='tomatodo.analyze'?30000:10000);
 requests.set(id,{resolve,reject,timer});postMessage({type:'rpc',id,method,params});
});
globalThis.DotLog=Object.freeze({
 render:view=>postMessage({type:'render',view}),
 getDay:date=>call('day.get',{date}),
 tomatodo:Object.freeze({scan:()=>call('tomatodo.scan'),import:()=>call('tomatodo.import'),compare:()=>call('tomatodo.compare'),analyze:()=>call('tomatodo.analyze')}),
 link:Object.freeze({status:()=>call('link.status'),enable:()=>call('link.enable'),disable:()=>call('link.disable'),
 startNext:()=>call('link.startNext'),endSegment:()=>call('link.endSegment'),
 saveNote:params=>call('link.saveNote',params),skipNote:params=>call('link.skipNote',params),reconcile:params=>call('link.reconcile',params)}),
 storage:Object.freeze({get:()=>call('storage.get'),set:data=>call('storage.set',data)}),
 onAction:handler=>{action=handler;},
 today:()=>{const d=new Date();return d.getFullYear()+'-'+String(d.getMonth()+1).padStart(2,'0')+'-'+String(d.getDate()).padStart(2,'0');}
});
onmessage=event=>{const m=event.data;
 if(m.type==='rpc-result'){const p=requests.get(m.id);if(!p)return;clearTimeout(p.timer);requests.delete(m.id);m.error?p.reject(new Error(m.error)):p.resolve(m.result);}
 if(m.type==='action')Promise.resolve().then(()=>action(m.action,m.values)).catch(e=>postMessage({type:'error',message:String(e)}));
};
`;
export function PluginPage({ plugin, embedded = false }: { plugin: LocalPlugin; embedded?: boolean }) {
  const permissionsKey = plugin.package.permissions.join("\0");
  const [view, setView] = useState<View | null>(null),
    [error, setError] = useState(""),
    [revision, setRevision] = useState(0);
  const worker = useRef<Worker | null>(null),
    timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const form = useRef<HTMLDivElement>(null);
  const draftIds = useRef<Record<string,string[]>>(Object.create(null));
  const draftKey = (id: string) => `dotlog.pluginDraft:${plugin.package.id}:${id}`;
  const noteDraft = (block: Block) => {
    if (!block.id?.startsWith("note:") || !plugin.package.permissions.includes("tomatodo:link")) return block.value;
    try { return localStorage.getItem(draftKey(block.id)) ?? block.value; } catch { return block.value; }
  };
  useEffect(() => {
    if (!view) return;
    const ids=view.blocks.map(b=>b.id).filter((id): id is string=>!!id?.startsWith("note:"));
    const scope=view.activeTab??"default";
    for (const id of draftIds.current[scope]??[]) if(!ids.includes(id)) { try {localStorage.removeItem(draftKey(id));} catch { /* draft storage unavailable */ } }
    draftIds.current[scope]=ids;
  }, [view, plugin.package.id]);
  const watchdog = (timeout=5000) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      worker.current?.terminate();
      setError("插件响应超时，已停止运行。可重试或在插件管理中停用。");
    }, timeout);
  };
  useEffect(() => {
    draftIds.current = Object.create(null);
    setView(null);
    setError("");
    if (!plugin.enabled) return;
    const url = URL.createObjectURL(
      new Blob([BOOTSTRAP, "\n", plugin.package.script], {
        type: "text/javascript",
      }),
    );
    const w = new Worker(url);
    worker.current = w;
    watchdog();
    w.onerror = () => {
      clearTimeout(timer.current);
      w.terminate();
      setError("插件脚本运行失败，请检查语法或联系插件作者。");
    };
    let messages = 0,
      windowStart = Date.now(),
      inFlight = 0,
      stopped = false;
    w.onmessage = (event) => {
      if (stopped) return;
      if (Date.now() - windowStart >= 1000) {
        messages = 0;
        windowStart = Date.now();
      }
      if (++messages > 60) {
        stopped = true;
        clearTimeout(timer.current);
        w.terminate();
        setError("插件发送请求过于频繁，已停止运行。");
        return;
      }
      try {
        const m = event.data;
        if (!m || typeof m !== "object") return;
        if (m.type === "render") {
          clearTimeout(timer.current);
          if (
            (JSON.stringify(m.view) ?? "").length > 262144 ||
            !Array.isArray(m.view?.blocks) ||
            m.view.blocks.length > 100
          ) {
            w.terminate();
            setError("插件页面超过限制");
            return;
          }
          if (!validPluginView(m.view)) {
            stopped = true;
            w.terminate();
            setError("插件页面格式不正确");
            return;
          }
          setView(m.view);
          setError("");
        }
        if (m.type === "error") {
          clearTimeout(timer.current);
          setError(String(m.message).slice(0, 2000));
        }
        if (m.type === "rpc") {
          if (inFlight >= 4) {
            w.postMessage({
              type: "rpc-result",
              id: m.id,
              error: "同时最多4个请求",
            });
            return;
          }
          if (JSON.stringify(m.params ?? {}).length > 131072) {
            w.postMessage({
              type: "rpc-result",
              id: m.id,
              error: "参数超过大小限制",
            });
            return;
          }
          inFlight++;
          void localPlugins
            .rpc(plugin.package.id, String(m.method), m.params ?? {})
            .then((result) =>
              w.postMessage({ type: "rpc-result", id: m.id, result }),
            )
            .catch((e) =>
              w.postMessage({
                type: "rpc-result",
                id: m.id,
                error: typeof e?.message === "string" ? e.message : String(e),
              }),
            )
            .finally(() => {
              inFlight--;
            });
        }
      } catch {
        clearTimeout(timer.current);
        w.terminate();
        setError("插件返回了无法处理的数据，已停止运行。");
      }
    };
    return () => {
      stopped = true;
      clearTimeout(timer.current);
      w.terminate();
      URL.revokeObjectURL(url);
      worker.current = null;
    };
  }, [plugin.package.id, plugin.package.script, plugin.enabled, permissionsKey, revision]);
  const dispatchAction = (action?:string) => {
    const values:Record<string,string>=Object.create(null);
    form.current?.querySelectorAll<HTMLInputElement|HTMLTextAreaElement>("input[name],textarea[name]").forEach(e=>{values[e.name]=e.value;});
    watchdog(30000);worker.current?.postMessage({type:"action",action,values});
  };
  return (
    <div className={embedded ? "plugin-page plugin-page--embedded" : "settings-page plugin-page"}>
      {!embedded && <header className="page-head">
        <h1>{plugin.package.name}</h1>
        <p>{plugin.package.description}</p>
        {!IS_TAURI && <span className="badge">浏览器预览 · 模拟记录</span>}
      </header>}
      {!plugin.enabled ? (
        <p>插件未启用，请到插件管理中启用。</p>
      ) : (
        <>
          {error && (
            <p role="alert" className="integration-card__error">
              {error}{" "}
              <button
                className="ghost-btn"
                onClick={() => setRevision((v) => v + 1)}
              >
                重新加载插件
              </button>
            </p>
          )}
          {!view && !error && <p role="status">正在加载插件…</p>}
          {view?.tabs && <div className="plugin-tabs" role="group" aria-label="插件功能">{view.tabs.map(t=><button key={t.id} aria-pressed={view.activeTab===t.id} onClick={()=>dispatchAction(t.action)}>{t.label}</button>)}</div>}
          {view && (
            <div className="settings-group plugin-view" ref={form}>
              {!embedded && <h2>{typeof view.title === "string" ? view.title : ""}</h2>}
              {view.blocks.map((b, i) => {
                const key = `${revision}:${b.id ?? i}`;
                switch (b.type) {
                  case "heading":
                    return <h3 key={key}>{b.text}</h3>;
                  case "markdown":
                    return <MemoContent key={key} content={b.text ?? ""} />;
                  case "stat":
                    return (
                      <div className="plugin-stat" key={key}>
                        <span>{b.label}</span>
                        <strong>
                          {b.value}
                        </strong>
                      </div>
                    );
                  case "input":
                  case "textarea":
                    return (
                      <label className="field plugin-field" key={key}>
                        <span className="field__label">{b.label}</span>
                        {b.type === "textarea" ? (
                          <textarea
                            className="field__input"
                            name={b.id}
                            defaultValue={noteDraft(b)}
                            onChange={event=>{if(b.id?.startsWith("note:")&&plugin.package.permissions.includes("tomatodo:link")) {
                              try {localStorage.setItem(draftKey(b.id),event.currentTarget.value);} catch {setError("草稿保存失败，请先保存本段备注再离开页面");}
                            }}}
                            maxLength={20000}
                            rows={5}
                          />
                        ) : (
                          <input
                            className="field__input"
                            type={b.inputType === "date" ? "date" : "text"}
                            name={b.id}
                            defaultValue={b.value}
                            maxLength={20000}
                          />
                        )}
                      </label>
                    );
                  case "button":
                    return (
                      <button
                        className={`plugin-action ${b.action === "end" || b.action === "next" || b.action === "enable" || b.action?.startsWith("save:") ? "primary-btn primary-btn--sm" : "ghost-btn"}`}
                        key={key}
                        disabled={b.disabled}
                        onClick={()=>dispatchAction(b.action)}
                      >
                        {b.text ?? "执行"}
                      </button>
                    );
                  default:
                    return (
                      <p style={{ whiteSpace: "pre-wrap" }} key={key}>
                        {b.text}
                      </p>
                    );
                }
              })}
            </div>
          )}
        </>
      )}
    </div>
  );
}
