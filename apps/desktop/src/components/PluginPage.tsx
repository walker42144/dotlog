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
  inputType?: "date" | "text";
};
type View = { title?: string; blocks: Block[] };
const BOOTSTRAP = `
const requests=new Map();let next=0;let action=()=>{};
const call=(method,params={})=>new Promise((resolve,reject)=>{
 const id=++next;const timer=setTimeout(()=>{requests.delete(id);reject(new Error('请求超时'));},10000);
 requests.set(id,{resolve,reject,timer});postMessage({type:'rpc',id,method,params});
});
globalThis.DotLog=Object.freeze({
 render:view=>postMessage({type:'render',view}),
 getDay:date=>call('day.get',{date}),
 storage:Object.freeze({get:()=>call('storage.get'),set:data=>call('storage.set',data)}),
 onAction:handler=>{action=handler;},
 today:()=>{const d=new Date();return d.getFullYear()+'-'+String(d.getMonth()+1).padStart(2,'0')+'-'+String(d.getDate()).padStart(2,'0');}
});
onmessage=event=>{const m=event.data;
 if(m.type==='rpc-result'){const p=requests.get(m.id);if(!p)return;clearTimeout(p.timer);requests.delete(m.id);m.error?p.reject(new Error(m.error)):p.resolve(m.result);}
 if(m.type==='action')Promise.resolve().then(()=>action(m.action,m.values)).catch(e=>postMessage({type:'error',message:String(e)}));
};
`;
export function PluginPage({ plugin }: { plugin: LocalPlugin }) {
  const [view, setView] = useState<View | null>(null),
    [error, setError] = useState(""),
    [revision, setRevision] = useState(0);
  const worker = useRef<Worker | null>(null),
    timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const form = useRef<HTMLDivElement>(null);
  const watchdog = () => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      worker.current?.terminate();
      setError("插件响应超时，已停止运行。可重试或在插件管理中停用。");
    }, 5000);
  };
  useEffect(() => {
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
          const allowed = [
            "heading",
            "text",
            "markdown",
            "stat",
            "input",
            "textarea",
            "button",
          ];
          const blocks = m.view.blocks;
          if (
            blocks.some(
              (b: Block) =>
                !b ||
                !allowed.includes(b.type) ||
                [b.text, b.value, b.id, b.label, b.action].some(
                  (v) => v !== undefined && typeof v !== "string",
                ),
            )
          ) {
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
      clearTimeout(timer.current);
      w.terminate();
      URL.revokeObjectURL(url);
      worker.current = null;
    };
  }, [plugin, revision]);
  return (
    <div className="settings-page">
      <header className="page-head">
        <h1>{plugin.package.name}</h1>
        <p>{plugin.package.description}</p>
        {!IS_TAURI && <span className="badge">浏览器预览 · 模拟记录</span>}
      </header>
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
          {view && (
            <div className="settings-group" ref={form}>
              <h2>{typeof view.title === "string" ? view.title : ""}</h2>
              {view.blocks.map((b, i) => {
                const key = `${revision}:${b.id ?? i}`;
                switch (b.type) {
                  case "heading":
                    return <h3 key={key}>{b.text}</h3>;
                  case "markdown":
                    return <MemoContent key={key} content={b.text ?? ""} />;
                  case "stat":
                    return (
                      <div className="integration-card" key={key}>
                        <span>{b.label}</span>
                        <strong style={{ display: "block", fontSize: 28 }}>
                          {b.value}
                        </strong>
                      </div>
                    );
                  case "input":
                  case "textarea":
                    return (
                      <label className="field" key={key}>
                        <span className="field__label">{b.label}</span>
                        {b.type === "textarea" ? (
                          <textarea
                            className="field__input"
                            name={b.id}
                            defaultValue={b.value}
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
                        className="primary-btn primary-btn--sm"
                        key={key}
                        onClick={() => {
                          const values: Record<string, string> =
                            Object.create(null);
                          form.current
                            ?.querySelectorAll<
                              HTMLInputElement | HTMLTextAreaElement
                            >("input[name],textarea[name]")
                            .forEach((e) => {
                              values[e.name] = e.value;
                            });
                          watchdog();
                          worker.current?.postMessage({
                            type: "action",
                            action: b.action,
                            values,
                          });
                        }}
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
