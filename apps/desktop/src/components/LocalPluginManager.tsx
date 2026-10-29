import guide from "../../../../docs/local-plugin-guide.md?raw";
import { MemoContent } from "./MemoContent";
import { useEffect, useRef, useState } from "react";
import {
  localPlugins,
  pluginsChanged,
  type PluginPackage,
  type LocalPlugin,
  PLUGINS_CHANGED,
} from "../lib/plugins";
import {
  friendlyMessage,
  ipc,
  toIpcError,
  type IntegrationView,
} from "../lib/ipc";
export function LocalPluginManager() {
  const [help, setHelp] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const [plugins, setPlugins] = useState<LocalPlugin[]>([]);
  const [builtins, setBuiltins] = useState<IntegrationView[]>([]);
  const [prefs, setPrefs] = useState<Record<string, boolean>>({});
  const [candidate, setCandidate] = useState<{
    text: string;
    pkg: PluginPackage;
  } | null>(null);
  const [replace, setReplace] = useState(false),
    [sidebar, setSidebar] = useState(true),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const [removing, setRemoving] = useState<string | null>(null);
  const load = async () => {
    const [p, b, s] = await Promise.all([
      localPlugins.list(),
      ipc.integrationList(),
      localPlugins.sidebar(),
    ]);
    setPlugins(p);
    setBuiltins(b);
    setPrefs(s ?? {});
  };
  useEffect(() => {
    const refresh = () => {
      void load().catch((e) => setError(friendlyMessage(toIpcError(e))));
    };
    refresh();
    window.addEventListener(PLUGINS_CHANGED, refresh);
    return () => window.removeEventListener(PLUGINS_CHANGED, refresh);
  }, []);
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await action();
      pluginsChanged();
      await load();
    } catch (e) {
      setError(friendlyMessage(toIpcError(e)));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="settings-group">
      <div className="plugin-manager-actions">
        <h2>本地插件与侧边栏</h2>
        <button
          className="ghost-btn"
          disabled={busy}
          onClick={() => file.current?.click()}
        >
          从本地安装
        </button>
        <button className="ghost-btn" onClick={() => setHelp((v) => !v)}>
          插件编写规范
        </button>
      </div>
      {help && (
        <div className="plugin-catalog">
          <MemoContent content={guide} />
        </div>
      )}
      <p className="modal__hint">
        安装 .dotlog-plugin.json
        文件。先核对作者和权限，安装后单独启用；更新也需要重新启用。
      </p>
      <input
        ref={file}
        type="file"
        accept=".json,.dotlog-plugin.json"
        hidden
        aria-label="选择本地插件包"
        onChange={async (e) => {
          const selected = e.target.files?.[0];
          e.target.value = "";
          if (!selected) return;
          try {
            if (selected.size > 262144) throw new Error("插件包不能超过256KB");
            const text = await selected.text();
            const pkg = JSON.parse(text) as PluginPackage;
            if (
              !pkg.id?.startsWith("local-") ||
              pkg.apiVersion !== 1 ||
              !Array.isArray(pkg.permissions) ||
              typeof pkg.script !== "string"
            )
              throw new Error("插件包格式不正确，请查看编写规范");
            setCandidate({ text, pkg });
            setReplace(false);
            setSidebar(true);
          } catch (e) {
            setError(String(e));
          }
        }}
      />
      {error && (
        <p className="integration-card__error" role="alert">
          {error}
        </p>
      )}
      {candidate && (
        <div className="plugin-catalog">
          <h3>
            {candidate.pkg.name} · {candidate.pkg.version}
          </h3>
          <p>
            作者：{candidate.pkg.author} · ID：{candidate.pkg.id}
          </p>
          <p>{candidate.pkg.description}</p>
          <p>
            请求权限：
            {candidate.pkg.permissions
              .map((p) =>
                p === "segments:read"
                  ? "读取所选日期全部记录（含标题和备注）"
                  : p === "storage"
                    ? "保存本插件的数据"
                    : `不支持：${p}`,
              )
              .join("、") || "无"}
          </p>
          <label>
            <input
              type="checkbox"
              checked={sidebar}
              onChange={(e) => setSidebar(e.target.checked)}
            />{" "}
            启用后在侧边栏展示
          </label>
          <br />
          <label>
            <input
              type="checkbox"
              checked={replace}
              onChange={(e) => setReplace(e.target.checked)}
            />{" "}
            替换同ID插件（保留插件数据，新版本需重新启用）
          </label>
          <div className="plugin-manager-actions">
            <button
              disabled={busy}
              className="primary-btn primary-btn--sm"
              onClick={() =>
                void run(async () => {
                  await localPlugins.install(candidate.text, replace, sidebar);
                  setCandidate(null);
                })
              }
            >
              确认安装
            </button>
            <button className="ghost-btn" onClick={() => setCandidate(null)}>
              取消
            </button>
          </div>
        </div>
      )}
      {builtins
        .filter((p) => p.installed)
        .map((p) => (
          <label className="setting-row" key={p.id}>
            <span>{p.name} · 侧边栏入口</span>
            <input
              type="checkbox"
              checked={prefs[p.id] ?? p.id === "memos"}
              disabled={busy}
              onChange={(e) =>
                void run(() => localPlugins.setSidebar(p.id, e.target.checked))
              }
            />
          </label>
        ))}
      {plugins.length === 0 && (
        <p className="modal__hint">
          尚未安装本地插件。可使用仓库 examples/plugins 中的“每日复盘”示例开始。
        </p>
      )}
      {plugins.map((p) => (
        <div className="integration-card" key={p.package.id}>
          <strong>
            {p.package.name} · v{p.package.version}
          </strong>
          <p>{p.package.description}</p>
          <p className="modal__hint">
            {p.package.author} ·{" "}
            {p.package.permissions.join("、") || "无额外权限"}
          </p>
          <div className="plugin-manager-actions">
            <label>
              <input
                type="checkbox"
                checked={p.enabled}
                disabled={busy}
                onChange={(e) =>
                  void run(() =>
                    localPlugins.change(p, { enabled: e.target.checked }),
                  )
                }
              />{" "}
              启用插件
            </label>
            <label>
              <input
                type="checkbox"
                checked={p.showInSidebar}
                disabled={busy}
                onChange={(e) =>
                  void run(() =>
                    localPlugins.change(p, { showInSidebar: e.target.checked }),
                  )
                }
              />{" "}
              显示在侧边栏
            </label>
            <button
              className="ghost-btn"
              disabled={!p.enabled}
              onClick={() =>
                window.dispatchEvent(
                  new CustomEvent("dotlog:open-plugin", {
                    detail: p.package.id,
                  }),
                )
              }
            >
              打开
            </button>
            <button
              className="ghost-btn"
              disabled={busy}
              onClick={() => setRemoving(p.package.id)}
            >
              移除
            </button>
          </div>
          {removing === p.package.id && (
            <p>
              移除插件并停止运行，数据保留以便重装。
              <button
                className="danger-btn"
                onClick={() =>
                  void run(async () => {
                    await localPlugins.change(p, {}, true);
                    setRemoving(null);
                  })
                }
              >
                确认移除
              </button>
              <button className="ghost-btn" onClick={() => setRemoving(null)}>
                取消
              </button>
            </p>
          )}
        </div>
      ))}
    </section>
  );
}
