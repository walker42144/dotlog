import type { ReactNode } from "react";
import {
  AnalysisIcon,
  HistoryIcon,
  IntegrationIcon,
  MemosIcon,
  SettingsIcon,
  TodayIcon
} from "./icons";

export type PageKey = "today" | "history" | "analysis" | "memos" | "integrations" | "settings" | `plugin:${string}`;

const ITEMS: Array<{ key: PageKey; label: string; icon: ReactNode; available: boolean }> = [
  { key: "today", label: "今天", icon: <TodayIcon />, available: true },
  { key: "history", label: "历史", icon: <HistoryIcon />, available: true },
  { key: "analysis", label: "分析", icon: <AnalysisIcon />, available: true },

  { key: "integrations", label: "插件管理", icon: <IntegrationIcon />, available: true },
  { key: "settings", label: "设置", icon: <SettingsIcon />, available: true }
];

interface SideNavProps {
  page: PageKey;
  onSelect: (page: PageKey) => void;
  plugins: Array<{id: string; name: string}>;
}

export function SideNav({ page, onSelect, plugins }: SideNavProps) {
  const items: typeof ITEMS = [...ITEMS.slice(0,3), ...plugins.map(p=>({key:(p.id==="memos"?"memos":`plugin:${p.id}`) as PageKey,label:p.name,icon:<MemosIcon/>,available:true})),...ITEMS.slice(3)];
  return (
    <nav className="sidenav" aria-label="主导航">
      {items.map((item) => (
        <button
          key={item.key}
          className="nav-item"
          type="button"
          disabled={!item.available}
          aria-current={page === item.key ? "page" : undefined}
          title={item.available ? item.label : `${item.label}（随后续版本开放）`}
          onClick={() => {
            if (item.available) onSelect(item.key);
          }}
        >
          {item.icon}
          <span className="nav-item__label">{item.label}</span>
        </button>
      ))}

      <div className="sidenav__foot">
        本地存储
        <br />
        v0.1.6
      </div>
    </nav>
  );
}
