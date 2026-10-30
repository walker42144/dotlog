import { AiAssistantPage } from "./components/AiAssistantPage";
import { useCallback, useEffect, useRef, useState } from "react";
import { PluginPage } from "./components/PluginPage";
import { localPlugins, PLUGINS_CHANGED, type LocalPlugin } from "./lib/plugins";
import { CommandPalette } from "./components/CommandPalette";
import { AnalysisPage } from "./components/AnalysisPage";
import { HistoryPage } from "./components/HistoryPage";
import { IntegrationsPage } from "./components/IntegrationsPage";
import { MemosPage } from "./components/MemosPage";
import { NotePopover } from "./components/NotePopover";
import { RecoveryDialog } from "./components/RecoveryDialog";
import { SegmentEditor, type EditorTarget } from "./components/SegmentEditor";
import { SettingsPage } from "./components/SettingsPage";
import { SideNav, type PageKey } from "./components/SideNav";
import { StatsAside } from "./components/StatsAside";
import { StatusPanel } from "./components/StatusPanel";
import { Timeline } from "./components/Timeline";
import { TitleBar } from "./components/TitleBar";
import { Toast, type ToastAction } from "./components/Toast";
import { useDay } from "./hooks/useDay";
import { useTimer } from "./hooks/useTimer";
import { configureDayStart, dayKeyOf, parseDayKey, formatDateHeading, formatDuration } from "./lib/format";
import { ipc, friendlyMessage, toIpcError, type SegmentView } from "./lib/ipc";

import { resolveTheme, themeMode, themePreference as parseThemePreference, type Theme } from "./lib/themes";
import { pluginNavigation } from "./lib/plugin-navigation";

interface ToastState {
  message: string;
  tone: "info" | "error";
}

export default function App() {
  const [theme, setTheme] = useState<Theme>(() =>
    resolveTheme(document.documentElement.dataset.theme, window.matchMedia("(prefers-color-scheme: dark)").matches)
  );
  const [page, setPage] = useState<PageKey>("today");
  const [externalPlugins,setExternalPlugins]=useState<LocalPlugin[]>([]);
  const [pluginNav,setPluginNav]=useState<Array<{id:string;name:string}>>([]);
  useEffect(()=>{
    let active=true;
    const load=()=>{ void Promise.all([localPlugins.list(),ipc.integrationList(),localPlugins.sidebar()]).then(([local,builtin,prefs])=>{
      if(!active)return;setExternalPlugins(local);
      setPluginNav(pluginNavigation(builtin, local, prefs ?? {}));
      setPage(current=>current.startsWith("plugin:")&&!builtin.some(p=>p.id!=="tomatodo"&&p.id===current.slice(7)&&p.installed&&p.enabled)&&!local.some(p=>p.package.id===current.slice(7)&&p.enabled)?"integrations":current==="memos"&&!builtin.some(p=>p.id==="memos"&&p.installed&&p.enabled)?"integrations":current);
    }).catch(()=>{if(active)setPluginNav([]);});};
    const open=(e:Event)=>{const id=(e as CustomEvent<string>).detail;if(typeof id==="string")setPage(`plugin:${id}`);};
    load();window.addEventListener(PLUGINS_CHANGED,load);window.addEventListener("dotlog:settings-changed",load);window.addEventListener("dotlog:open-plugin",open);
    return()=>{active=false;window.removeEventListener(PLUGINS_CHANGED,load);window.removeEventListener("dotlog:settings-changed",load);window.removeEventListener("dotlog:open-plugin",open);};
  },[]);
  const [toast, setToast] = useState<ToastState | null>(null);
  const notify = useCallback((message: string, tone: "info" | "error" = "info") => setToast({ message, tone }), []);
  const [editing, setEditing] = useState<EditorTarget | null>(null);
  const timer = useTimer();
  const day = useDay(timer.dayVersion);
  const [clockTick, setClockTick] = useState(0);
  const [themePreference, setThemePreference] = useState(() => {
    try { return parseThemePreference(localStorage.getItem("dotlog.theme")); } catch { return "system"; }
  });
  const mainRef = useRef<HTMLElement>(null);
  const [motion, setMotion] = useState(() => document.documentElement.dataset.motion === "reduced" ? "reduced" : "system");
  useEffect(() => {
    document.documentElement.dataset.motion = motion;
    try { localStorage.setItem("dotlog.motion", motion); } catch { /* Optional startup cache. */ }
  }, [motion]);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    let animation: Animation | undefined;
    if (motion !== "reduced" && !media.matches) {
      animation = mainRef.current?.animate([{ opacity: 0.65 }, { opacity: 1 }], { duration: 160, easing: "ease-out" });
    }
    const cancel = () => animation?.cancel();
    media.addEventListener("change", cancel);
    return () => { cancel(); media.removeEventListener("change", cancel); };
  }, [page, motion]);
  const dates = formatDateHeading(parseDayKey(day.day?.date ?? dayKeyOf(Date.now())));
  useEffect(() => {
    const id = window.setInterval(() => setClockTick((v) => v + 1), 15_000);
    return () => window.clearInterval(id);
  }, []);
  useEffect(() => {
    let active = true;
    const load = () => { void ipc.settingsGet("app").then((settings) => {
      if (!active) return;
      configureDayStart(Number(settings["day.start_minutes"] ?? 240));
      setClockTick((v) => v + 1);
      setMotion(settings["ui.motion"] === "reduced" ? "reduced" : "system");
      const preference = parseThemePreference(settings["ui.theme"]);
      setThemePreference(preference);
      setTheme(resolveTheme(preference, window.matchMedia("(prefers-color-scheme: dark)").matches));
    }).catch((error: unknown) => notify(friendlyMessage(toIpcError(error)), "error")); };
    load(); window.addEventListener("dotlog:settings-changed", load);
    return () => { active = false; window.removeEventListener("dotlog:settings-changed", load); };
  }, []);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => { if (themePreference === "system") setTheme(media.matches ? "dark" : "light"); };
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, [themePreference]);
  void clockTick;

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("dotlog.theme", themePreference);
    } catch {
      // 隐私模式下写入失败不影响使用。
    }
  }, [theme, themePreference]);

  useEffect(() => {
    if (!timer.error) return;
    setToast({ message: friendlyMessage(timer.error), tone: "error" });
    timer.clearError();
  }, [timer.error, timer.clearError]);

  useEffect(() => {
    if (!day.error) return;
    setToast({ message: friendlyMessage(day.error), tone: "error" });
  }, [day.error]);

  useEffect(() => {
    if (!toast) return;
    const id = window.setTimeout(() => setToast(null), 4_000);
    return () => window.clearTimeout(id);
  }, [toast]);

  // Ctrl+Z 仅在撤销窗口内生效；输入框内不拦截。
  useEffect(() => {
    if (timer.undoRemaining <= 0) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "z") return;
      const target = event.target;
      if (target instanceof HTMLElement && /^(INPUT|TEXTAREA)$/.test(target.tagName)) return;
      event.preventDefault();
      timer.undo();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [timer.undoRemaining, timer.undo]);

  const toggleTheme = useCallback(
    () => {
      const next = themeMode(theme) === "dark" ? "light" : "dark";
      setTheme(next); setThemePreference(next);
      void ipc.settingsPatch("app", [{ key: "ui.theme", value: next }]).then(() => window.dispatchEvent(new Event("dotlog:settings-changed"))).catch((error: unknown) => {
        setTheme(theme); setThemePreference(themePreference);
        notify(friendlyMessage(toIpcError(error)), "error");
      });
    },
    [theme, themePreference, notify]
  );

  const openSegmentEditor = useCallback((segment: SegmentView, next: SegmentView | null) => {
    void Promise.all([ipc.segmentGet(segment.id), next ? ipc.segmentGet(next.id) : Promise.resolve(null)]).then(([original, nextSegment]) => {
      setEditing({ mode: "edit", segment: original, nextSegment });
    }).catch((e: unknown) => setToast({ message: e instanceof Error ? e.message : "读取记录失败，请重试", tone: "error" }));
  }, []);

  const openBackfill = useCallback((startAt: number, endAt: number) => {
    setEditing({ mode: "backfill", startAt, endAt });
  }, []);

  const state = timer.state;
  const recording = state?.phase === "recording";
  const statusLabel =
    state === null
      ? "载入中"
      : recording
        ? `记录中 · ${formatDuration(timer.elapsed)}`
        : state.paused
          ? `暂停中 · ${formatDuration(timer.elapsed)}`
          : "未开始";

  const undoToast =
    timer.undoableLabel && timer.undoRemaining > 0
      ? {
          message: `${timer.undoableLabel} · ${timer.undoRemaining} 秒内可撤销`,
          tone: "info" as const,
          action: { label: "撤销", onClick: timer.undo }
        }
      : null;
  const visibleToast: (ToastState & { action?: ToastAction }) | null = toast ?? undoToast;

  return (
    <div className="app">
      <CommandPalette commands={[
        { label: "今天", description: "查看今天的时间线", action: () => setPage("today") },
        { label: "历史", description: "查询和修正过去的记录", action: () => setPage("history") },
        { label: "分析", description: "查看统计与分类", action: () => setPage("analysis") },
        { label: "Memos", description: "使用插件 Token 查看笔记", action: () => setPage("memos") },
        { label: "插件管理", description: "添加、移除或配置插件", action: () => setPage("integrations") },
        { label: "设置与备份", description: "调整规则和保护数据", action: () => setPage("settings") },
        { label: "立即备份", description: "为当前数据库创建快照", action: () => { void ipc.backupNow().then(() => setToast({ message: "已生成备份", tone: "info" })).catch(() => setToast({ message: "备份失败，请重试", tone: "error" })); } }
      ]} />
      <TitleBar
        dateLabel={dates.compact}
        statusLabel={statusLabel}
        statusTone={recording ? "recording" : state?.paused ? "paused" : "idle"}
        theme={themeMode(theme)}
        onToggleTheme={toggleTheme}
      />

      <div className={`app-body${page === "today" ? "" : " app-body--plain"}`}>
        <SideNav page={page} onSelect={setPage} plugins={pluginNav} />

        <main className="app-main" ref={mainRef}>
          {page === "today" && (
            <>
              <header className="page-head">
                <h1>今天</h1>
                <p>{dates.full} · 本地时间</p>
              </header>

              <div className="status-wrap">
                <StatusPanel
                  state={state}
                  elapsed={timer.elapsed}
                  pending={timer.pending}
                  onStart={timer.start}
                  onPunch={timer.punch}
                  onPause={timer.pause}
                  onFinish={timer.finish}
                />

                {timer.pendingNote && (
                  <NotePopover
                    key={timer.pendingNote.id}
                    segment={timer.pendingNote}
                    saving={timer.pending}
                    onSave={timer.saveNote}
                    onSkip={timer.skipNote}
                  />
                )}
              </div>

              <Timeline
                day={day.day}
                loading={day.loading}
                elapsed={timer.elapsed}
                onStart={timer.start}
                onEditSegment={openSegmentEditor}
                onBackfillGap={openBackfill}
              />
            </>
          )}

          {page === "history" && (
            <HistoryPage dayVersion={timer.dayVersion} onEditSegment={openSegmentEditor} />
          )}

          {page === "analysis" && <AnalysisPage dayVersion={timer.dayVersion} />}

          {page === "memos" && (
            <MemosPage onGoIntegrations={() => setPage("integrations")} />
          )}

          {page === "integrations" && (
            <IntegrationsPage
              onNotify={notify}
              onDataChanged={timer.refresh}
            />
          )}

          {page.startsWith("plugin:") && (page === "plugin:ai" ? <AiAssistantPage onNotify={notify} onDataChanged={timer.refresh}/> : externalPlugins.find(p=>p.package.id===page.slice(7)&&p.enabled) ? <PluginPage key={page} plugin={externalPlugins.find(p=>p.package.id===page.slice(7))!}/> : <p>插件未安装或未启用，请在插件管理中检查。</p>)}

          {page === "settings" && (
            <SettingsPage
              onNotify={notify}
              onDataChanged={timer.refresh}
            />
          )}
        </main>

        {page === "today" && (
          <StatsAside
            state={state}
            elapsed={timer.elapsed}
            dateLabel={dates.compact}
            day={day.day}
            loading={day.loading}
          />
        )}
      </div>

      {editing && (
        <SegmentEditor
          target={editing}
          pending={timer.pending}
          onClose={() => setEditing(null)}
          onSave={timer.saveSegment}
          onBackfill={timer.backfillSegment}
          onDelete={timer.deleteSegment}
          onSplit={timer.splitSegment}
          onMerge={timer.mergeSegments}
        />
      )}

      {timer.recovery && (
        <RecoveryDialog
          info={timer.recovery}
          busy={timer.resolving}
          onResolve={timer.resolveRecovery}
        />
      )}

      {visibleToast && (
        <Toast
          message={visibleToast.message}
          tone={visibleToast.tone}
          action={visibleToast.action}
        />
      )}
    </div>
  );
}
