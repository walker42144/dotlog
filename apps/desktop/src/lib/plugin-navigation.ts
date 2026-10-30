import type { LocalPlugin } from "./plugins";
import type { IntegrationView } from "./ipc";
export const TOMATODO_LINK_ID = "local-tomatodo-link";
export function pluginNavigation(builtin: IntegrationView[], local: LocalPlugin[], prefs: Record<string, boolean>) {
  const nav = builtin.filter(p => p.installed && p.enabled && p.id !== "tomatodo" && (prefs[p.id] ?? p.id === "memos"))
    .map(p => ({ id: p.id, name: p.id === "memos" ? "Memos" : p.name }));
  return nav.concat(local.filter(p => p.enabled && p.showInSidebar)
    .map(p => ({ id: p.package.id, name: p.package.name })));

}
