// Worker output is untrusted, including optional metadata and React keys.
export function validPluginView(view: unknown): boolean {
  if (!view || typeof view !== "object") return false;
  const v = view as Record<string, unknown>;
  if ((v.title !== undefined && typeof v.title !== "string") ||
      (v.activeTab !== undefined && typeof v.activeTab !== "string") ||
      !Array.isArray(v.blocks) || v.blocks.length > 100) return false;
  const allowed = ["heading", "text", "markdown", "stat", "input", "textarea", "button"];
  const ids = new Set<string>();
  for (const block of v.blocks) {
    if (!block || typeof block !== "object" || !allowed.includes(block.type) ||
        (block.disabled !== undefined && typeof block.disabled !== "boolean") ||
        [block.text, block.value, block.id, block.label, block.action].some(value => value !== undefined && typeof value !== "string") ||
        (block.inputType !== undefined && !["text", "date"].includes(block.inputType))) return false;
    if (block.id !== undefined) {
      if (!block.id || ids.has(block.id)) return false;
      ids.add(block.id);
    }
  }
  if (v.activeTab !== undefined && v.tabs === undefined) return false;
  if (v.tabs !== undefined) {
    if (!Array.isArray(v.tabs) || v.tabs.length > 5) return false;
    const tabIds = new Set<string>();
    for (const tab of v.tabs) {
      if (!tab || [tab.id, tab.label, tab.action].some(value => typeof value !== "string" || !value || value.length > 100) || tabIds.has(tab.id)) return false;
      tabIds.add(tab.id);
    }
    if (v.activeTab !== undefined && !tabIds.has(v.activeTab as string)) return false;
  }
  return true;
}
