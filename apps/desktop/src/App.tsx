import { useCallback, useEffect, useState } from "react";
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
import { formatDateHeading, formatDuration } from "./lib/format";
import { friendlyMessage, type SegmentView } from "./lib/ipc";

type Theme = "light" | "dark";

interface ToastState {
  message: string;
  tone: "info" | "error";
}

export default function App() {
  const [theme, setTheme] = useState<Theme>(() =>
    document.documentElement.dataset.theme === "dark" ? "dark" : "light"
  );
  const [page, setPage] = useState<PageKey>("today");
  const [toast, setToast] = useState<ToastState | null>(null);
  const [editing, setEditing] = useState<EditorTarget | null>(null);
  const timer = useTimer();
  const day = useDay(timer.dayVersion);
  const dates = formatDateHeading();

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("dotlog.theme", theme);
    } catch {
      // 隐私模式下写入失败不影响使用。
    }
  }, [theme]);

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
    () => setTheme((current) => (current === "dark" ? "light" : "dark")),
    []
  );

  const openSegmentEditor = useCallback((segment: SegmentView, next: SegmentView | null) => {
    setEditing({ mode: "edit", segment, nextSegment: next });
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
          ? "暂停中"
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
      <TitleBar
        dateLabel={dates.compact}
        statusLabel={statusLabel}
        statusTone={recording ? "recording" : state?.paused ? "paused" : "idle"}
        theme={theme}
        onToggleTheme={toggleTheme}
      />

      <div className={`app-body${page === "today" ? "" : " app-body--plain"}`}>
        <SideNav page={page} onSelect={setPage} />

        <main className="app-main">
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
              onNotify={(message, tone) => setToast({ message, tone: tone ?? "info" })}
              onDataChanged={timer.refresh}
            />
          )}

          {page === "settings" && (
            <SettingsPage
              onThemeChange={setTheme}
              onNotify={(message, tone) => setToast({ message, tone: tone ?? "info" })}
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
