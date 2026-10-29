import { invoke } from "@tauri-apps/api/core";
import { ipc, IS_TAURI } from "./ipc";
export interface PluginPackage {
  id: string;
  name: string;
  version: string;
  apiVersion: number;
  author: string;
  description: string;
  permissions: string[];
  script: string;
}
export interface LocalPlugin {
  package: PluginPackage;
  enabled: boolean;
  showInSidebar: boolean;
}
export const PLUGINS_CHANGED = "dotlog:plugins-changed";
export const pluginsChanged = () =>
  window.dispatchEvent(new Event(PLUGINS_CHANGED));
const get = <T>(key: string, fallback: T): T => {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "null") ?? fallback;
  } catch {
    return fallback;
  }
};
async function call<T>(
  name: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  if (IS_TAURI) return invoke<T>(name, args);
  const list = get<LocalPlugin[]>("dotlog.preview.plugins", []);
  const prefs = get<Record<string, boolean>>("dotlog.preview.sidebar", {});
  const p = list.find((p) => p.package.id === args.id);
  let result: unknown = null;
  switch (name) {
    case "local_plugin_list":
      result = list;
      break;
    case "local_plugin_install": {
      const pkg = JSON.parse(String(args.text)) as PluginPackage;
      if (list.some((p) => p.package.id === pkg.id) && !args.replace)
        throw new Error("同ID插件已安装，请选择替换更新");
      result = {
        package: pkg,
        enabled: false,
        showInSidebar: args.showInSidebar,
      };
      localStorage.setItem(
        "dotlog.preview.plugins",
        JSON.stringify([
          ...list.filter((p) => p.package.id !== pkg.id),
          result,
        ]),
      );
      break;
    }
    case "local_plugin_change":
      if (!p) throw new Error("插件未安装");
      localStorage.setItem(
        "dotlog.preview.plugins",
        JSON.stringify(
          args.remove
            ? list.filter((x) => x !== p)
            : list.map((x) =>
                x === p
                  ? {
                      ...p,
                      enabled: args.enabled,
                      showInSidebar: args.showInSidebar,
                    }
                  : x,
              ),
        ),
      );
      break;
    case "plugin_sidebar_get":
      result = prefs;
      break;
    case "plugin_sidebar_set":
      localStorage.setItem(
        "dotlog.preview.sidebar",
        JSON.stringify({ ...prefs, [String(args.id)]: args.visible }),
      );
      break;
    case "local_plugin_rpc": {
      const permission =
        args.method === "day.get"
          ? "segments:read"
          : args.method === "storage.get" || args.method === "storage.set"
            ? "storage"
            : "unsupported";
      if (!p?.enabled || !p.package.permissions.includes(permission))
        throw new Error("插件未启用或未授权此操作");
      if (args.method === "day.get")
        result = await ipc.dayGet(
          String((args.params as { date: string }).date),
        );
      else if (args.method === "storage.get")
        result = get(`dotlog.preview.data:${args.id}`, null);
      else if (args.method === "storage.set") {
        if (JSON.stringify(args.params).length > 65536)
          throw new Error("插件数据超过64KB");
        localStorage.setItem(
          `dotlog.preview.data:${args.id}`,
          JSON.stringify(args.params),
        );
      }
      break;
    }
  }
  return result as T;
}
export const localPlugins = {
  list: () => call<LocalPlugin[]>("local_plugin_list"),
  install: (text: string, replace: boolean, showInSidebar: boolean) =>
    call<LocalPlugin>("local_plugin_install", { text, replace, showInSidebar }),
  change: (p: LocalPlugin, patch: Partial<LocalPlugin>, remove = false) =>
    call<void>("local_plugin_change", {
      id: p.package.id,
      enabled: patch.enabled ?? p.enabled,
      showInSidebar: patch.showInSidebar ?? p.showInSidebar,
      remove,
    }),
  sidebar: () => call<Record<string, boolean> | null>("plugin_sidebar_get"),
  setSidebar: (id: string, visible: boolean) =>
    call<void>("plugin_sidebar_set", { id, visible }),
  rpc: (id: string, method: string, params: unknown) =>
    call<unknown>("local_plugin_rpc", { id, method, params }),
};
