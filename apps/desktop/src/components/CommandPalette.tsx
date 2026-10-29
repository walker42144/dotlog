import { createPortal } from "react-dom";
import "../styles/command-palette.css";
import { useEffect, useMemo, useRef, useState } from "react";
export interface QuickCommand { label: string; description: string; action: () => void }
export function CommandPalette({ commands }: { commands: QuickCommand[] }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const previous = useRef<Element | null>(null);
  const results = useMemo(() => commands.filter((c) => `${c.label} ${c.description}`.toLowerCase().includes(query.toLowerCase())), [commands, query]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setOpen((v) => !v); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  useEffect(() => {
    if (open) { previous.current = document.activeElement; setQuery(""); input.current?.focus(); }
    else if (previous.current instanceof HTMLElement) previous.current.focus();
  }, [open]);
  const run = (command: QuickCommand) => { setOpen(false); command.action(); };
  return <><button className="quick-command-trigger" onClick={() => setOpen(true)} title="快捷操作 Ctrl+K">快捷操作 <kbd>Ctrl K</kbd></button>
  {open && createPortal(<div className="command-overlay" onClick={() => setOpen(false)}><div className="command-panel" role="dialog" aria-modal="true" aria-label="快捷操作" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => {
    if (e.key === "Escape") { e.stopPropagation(); setOpen(false); }
    if (e.key === "Enter" && e.target === input.current && results[0]) { e.preventDefault(); run(results[0]); }
    if (e.key === "Tab") {
      const focusables = e.currentTarget.querySelectorAll<HTMLElement>("input,button");
      const first = focusables[0], last = focusables[focusables.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
      if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
    }
  }}><h2>快捷操作</h2><input ref={input} className="field__input" aria-label="搜索操作" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="输入页面或操作名称…" />{results.map((command) => <button key={command.label} className="command-option" onClick={() => run(command)}><strong>{command.label}</strong><span>{command.description}</span></button>)}{!results.length && <p>没有匹配的操作</p>}<p className="modal__hint">Enter 执行第一项 · Tab 选择 · Esc 关闭</p></div></div>, document.body)}</>;
}
