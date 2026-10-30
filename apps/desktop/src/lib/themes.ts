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
  { value: "glass", label: "轻透玻璃", color: "#245b83" },
  { value: "acrylic", label: "磨砂亚克力", color: "#5b4a94" },
  { value: "obsidian", label: "深色悬浮", color: "#a8c9ff" },
  { value: "ceramic", label: "柔光陶瓷", color: "#3e5d57" },
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
  return ["dark", "forest", "ocean", "console", "obsidian"].includes(theme) ? "dark" : "light";
}

export const THEME_DETAILS: Record<ThemePreference, string> = {
  system: "随系统切换深浅", light: "明亮 · 简洁", dark: "低光 · 专注", paper: "暖白 · 纸面",
  forest: "深绿 · 安静", ocean: "深蓝 · 沉稳", lavender: "浅紫 · 柔和", rose: "浅玫 · 温润",
  grid: "方格 · 手写页眉", folio: "索引 · 层叠纸张", console: "金属 · 仪表屏",
  glass: "清透 · 悬浮光边", acrylic: "磨砂 · 柔焦微粒", obsidian: "烟玻璃 · 深色悬浮", ceramic: "乳白 · 柔光曲面",
};
