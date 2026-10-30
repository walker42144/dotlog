export const THEMES = [
  { value: "system", label: "跟随系统", color: "#8b909a" },
  { value: "light", label: "清浅", color: "#315b9b" },
  { value: "dark", label: "墨夜", color: "#9cbbeb" },
  { value: "paper", label: "暖纸", color: "#946132" },
  { value: "forest", label: "深林", color: "#9bc99b" },
  { value: "ocean", label: "深海", color: "#85baff" },
  { value: "lavender", label: "雾紫", color: "#7954ae" },
  { value: "rose", label: "蔷薇", color: "#a94c69" },
  { value: "grid", label: "方格手账", color: "#286658" },
  { value: "folio", label: "翻页档案", color: "#4b50a0" },
  { value: "console", label: "仪表台", color: "#efba69" },
  { value: "glass", label: "轻透浮层", color: "#245b83" },
] as const;
export type ThemePreference = typeof THEMES[number]["value"];
export type Theme = Exclude<ThemePreference, "system">;
export function themePreference(value: unknown): ThemePreference {
  return THEMES.some(t => t.value === value) ? value as ThemePreference : "system";
}
export function resolveTheme(value: unknown, dark: boolean): Theme {
  const preference = themePreference(value);
  return preference === "system" ? dark ? "dark" : "light" : preference;
}
export function themeMode(theme: Theme): "light" | "dark" {
  return ["dark", "forest", "ocean", "console"].includes(theme) ? "dark" : "light";
}
