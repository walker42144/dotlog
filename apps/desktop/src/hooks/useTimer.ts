import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { formatDuration } from "../lib/format";
import {
  IS_TAURI,
  ipc,
  toIpcError,
  type BackfillInput,
  type IpcError,
  type RecoveryAction,
  type RecoveryInfo,
  type SegmentPatch,
  type SegmentView,
  type TimerState
} from "../lib/ipc";

// 与 src-tauri/src/lib.rs 的 QUICK_PUNCH_EVENT / QUICK_PAUSE_EVENT 保持一致。
const QUICK_PUNCH_EVENT = "dotlog://quick-punch";
const QUICK_PAUSE_EVENT = "dotlog://quick-pause";

export interface TimerController {
  state: TimerState | null;
  lastClosed: SegmentView | null;
  pendingNote: SegmentView | null;
  /** 撤销剩余秒数；0 表示不可撤销。 */
  undoRemaining: number;
  /** 可撤销操作的描述（打点/补录/编辑/删除等），null 表示当前不可撤销。 */
  undoableLabel: string | null;
  /** 每次数据变更自增，供日视图等派生数据失效。 */
  dayVersion: number;
  elapsed: number;
  pending: boolean;
  error: IpcError | null;
  /** 启动恢复提示；`required` 为 true 时必须先处理才能继续记录。 */
  recovery: RecoveryInfo | null;
  resolving: boolean;
  resolveRecovery: (action: RecoveryAction) => void;
  /** 外部数据变更（导入 / 恢复备份）后手动触发派生视图刷新。 */
  refresh: () => void;
  start: () => void;
  punch: () => void;
  pause: () => void;
  finish: () => void;
  undo: () => void;
  saveNote: (title: string, note: string) => void;
  skipNote: () => void;
  clearError: () => void;
  /** 以下编辑操作直接抛 IpcError 给调用方（编辑器内联展示），不进全局错误 toast。 */
  saveSegment: (id: string, revision: number, patch: SegmentPatch) => Promise<SegmentView>;
  backfillSegment: (input: BackfillInput) => Promise<SegmentView>;
  deleteSegment: (id: string) => Promise<void>;
  splitSegment: (id: string, at: number) => Promise<SegmentView[]>;
  mergeSegments: (ids: string[]) => Promise<SegmentView>;
}

/** 本地秒表：以 active.startAt 为基准，每秒刷新一次。 */
function useElapsed(state: TimerState | null): number {
  const [now, setNow] = useState(Date.now());
  const anchor = useRef({ state, at: Date.now() });
  if (anchor.current.state !== state) anchor.current = { state, at: Date.now() };
  useEffect(() => {
    if (!state?.active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [state?.active?.id]);
  return Math.max(0, (state?.elapsedSeconds ?? 0) + (state?.active ? Math.floor(Math.max(0, now - anchor.current.at) / 1000) : 0));
}

/** 撤销倒计时；到 0 后自动失效。 */
function useCountdown(deadline: number | null): number {
  const [remaining, setRemaining] = useState(0);

  useEffect(() => {
    if (deadline === null) {
      setRemaining(0);
      return;
    }
    const tick = () => {
      setRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1_000)));
    };
    tick();
    const id = window.setInterval(tick, 500);
    return () => window.clearInterval(id);
  }, [deadline]);

  return remaining;
}

export function useTimer(): TimerController {
  const [state, setState] = useState<TimerState | null>(null);
  const [lastClosed, setLastClosed] = useState<SegmentView | null>(null);
  const [pendingNote, setPendingNote] = useState<SegmentView | null>(null);
  const [undoDeadline, setUndoDeadline] = useState<number | null>(null);
  const [undoableLabel, setUndoableLabel] = useState<string | null>(null);
  const [dayVersion, setDayVersion] = useState(0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [recovery, setRecovery] = useState<RecoveryInfo | null>(null);
  const [resolving, setResolving] = useState(false);
  // 事件回调需要读到最新状态，避免注册时的闭包旧值。
  const stateRef = useRef<TimerState | null>(null);
  const running = useRef(false);
  const undoWindowMs = useRef(15_000);
  const promptOnPunch = useRef(true);

  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  useEffect(() => {
    let cancelled = false;
    void ipc
      .getState()
      .then((next) => {
        if (!cancelled) setState(next);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(toIpcError(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const run = useCallback(async (action: () => Promise<void>) => {
    if (running.current) return;
    running.current = true;
    setPending(true);
    try {
      await action();
      setError(null);
    } catch (cause) {
      setError(toIpcError(cause));
    } finally {
      running.current = false;
      setPending(false);
    }
  }, []);

  const refreshDay = useCallback(() => setDayVersion((value) => value + 1), []);
  const refresh = useCallback(() => {
    setLastClosed(null); setPendingNote(null); setUndoDeadline(null); setUndoableLabel(null);
    void Promise.all([ipc.getState(), ipc.recoveryGet(), ipc.settingsGet("app"), ipc.getPendingNote()]).then(([next, info, settings, note]) => {
      setState(next); stateRef.current = next; setRecovery(info.required ? info : null);
      undoWindowMs.current = Number(settings["punch.undo_window_ms"] ?? 15_000);
      promptOnPunch.current = settings["notify.on_punch"] !== false;
      setPendingNote(note);
      refreshDay();
    }).catch((cause: unknown) => setError(toIpcError(cause)));
  }, [refreshDay]);

  useEffect(() => {
    let active = true;
    const onChanged = () => { void ipc.settingsGet("app").then(settings => {
      if (!active) return;
      undoWindowMs.current = Number(settings["punch.undo_window_ms"] ?? 15_000);
      promptOnPunch.current = settings["notify.on_punch"] !== false;
      refreshDay();
    }).catch((cause: unknown) => { if (active) setError(toIpcError(cause)); }); };
    window.addEventListener("dotlog:settings-changed", onChanged);
    return () => { active = false; window.removeEventListener("dotlog:settings-changed", onChanged); };
  }, [refreshDay]);
  useEffect(() => { refresh(); }, [refresh]);
  useEffect(() => {
    const check = () => { void ipc.recoveryGet().then((info) => setRecovery(info.required ? info : null)).catch((cause: unknown) => setError(toIpcError(cause))); };
    const id = window.setInterval(check, 15_000);
    return () => window.clearInterval(id);
  }, []);

  // 启动恢复提示：后端在 setup 阶段已完成检测并缓存结果。
  useEffect(() => {
    let cancelled = false;
    void ipc
      .recoveryGet()
      .then((info) => {
        if (!cancelled) setRecovery(info.required ? info : null);
      })
      .catch((cause: unknown) => { if (!cancelled) setError(toIpcError(cause)); });
    return () => {
      cancelled = true;
    };
  }, []);

  const resolveRecovery = useCallback(
    (action: RecoveryAction) => {
      setResolving(true);
      void (async () => {
        try {
          const next = await ipc.recoveryResolve(action);
          setState(next);
          setRecovery(null);
          setLastClosed(null);
          setPendingNote(await ipc.getPendingNote());
          setUndoDeadline(null);
          setUndoableLabel(null);
          refreshDay();
          setError(null);
        } catch (cause) {
          setError(toIpcError(cause));
        } finally {
          setResolving(false);
        }
      })();
    },
    [refreshDay]
  );

  const start = useCallback(() => {
    void run(async () => {
      const next = await ipc.start();
      setState(next);
      setLastClosed(null);
      setPendingNote(null);
      setUndoDeadline(null);
      setUndoableLabel(null);
      refreshDay();
    });
  }, [run, refreshDay]);

  const close = useCallback(
    async (command: () => Promise<Awaited<ReturnType<typeof ipc.punch>>>) => {
      const result = await command();
      setLastClosed(result.closedSegment);
      setPendingNote(result.pendingNote && promptOnPunch.current ? result.closedSegment : null);
      setUndoDeadline(result.undoToken ? Date.now() + undoWindowMs.current : null);
      const seconds = result.closedSegment.durationSeconds;
      setUndoableLabel(
        result.undoToken
          ? seconds < 15
            ? `刚记录了 ${formatDuration(seconds)}`
            : `已记录 ${formatDuration(seconds)}`
          : null
      );
      setState(await ipc.getState());
      refreshDay();
    },
    [refreshDay]
  );

  const punch = useCallback(() => {
    void run(() => close(() => ipc.punch()));
  }, [run, close]);

  const pause = useCallback(() => {
    void run(async () => {
      await close(() => ipc.pause());
      setPendingNote(null);
      setUndoableLabel("已暂停，暂停时间不计入记录");
    });
  }, [run, close]);

  const finish = useCallback(() => {
    void run(() => close(() => ipc.finish()));
  }, [run, close]);

  // 托盘「打点」与全局快捷键共用：记录中则打点，否则开始/继续记录。
  useEffect(() => {
    if (!IS_TAURI) return;
    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const register = async () => {
      const offPunch = await listen<Awaited<ReturnType<typeof ipc.punch>> | null>(QUICK_PUNCH_EVENT, (event) => {
        refresh();
        const result = event.payload;
        if (result) {
          setLastClosed(result.closedSegment);
          setPendingNote(result.pendingNote && promptOnPunch.current ? result.closedSegment : null);
          setUndoableLabel("已记录时间段");
          setUndoDeadline(Date.now() + undoWindowMs.current);
        }
      });
      if (disposed) {
        offPunch();
        return;
      }
      unlisteners.push(offPunch);

      const offPause = await listen(QUICK_PAUSE_EVENT, () => { refresh(); });
      if (disposed) {
        offPause();
        return;
      }
      unlisteners.push(offPause);
      const offLink = await listen("dotlog://link-changed", () => { refresh(); });
      if (disposed) { offLink(); return; }
      unlisteners.push(offLink);
    };

    void register().catch(() => {
      // 事件系统不可用时静默降级，界面按钮仍可操作。
    });

    return () => {
      disposed = true;
      for (const off of unlisteners) off();
    };
  }, [refresh]);

  const undo = useCallback(() => {
    void run(async () => {
      await ipc.undo();
      setLastClosed(null);
      setPendingNote(null);
      setUndoDeadline(null);
      setUndoableLabel(null);
      setState(await ipc.getState());
      refreshDay();
    });
  }, [run, refreshDay]);

  // 编辑类操作统一入口：成功后开启 15 秒撤销窗口；错误抛给编辑器内联展示。
  const runEdit = useCallback(
    async <T,>(label: string, action: () => Promise<T>): Promise<T> => {
      if (running.current) throw toIpcError(new Error("正在处理上一项操作"));
      running.current = true;
      setPending(true);
      try {
        const result = await action();
        setLastClosed(null);
        setPendingNote(null);
        setUndoableLabel(label);
        setUndoDeadline(Date.now() + undoWindowMs.current);
        setState(await ipc.getState());
        refreshDay();
        setError(null);
        return result;
      } catch (cause) {
        throw toIpcError(cause);
      } finally {
        running.current = false;
        setPending(false);
      }
    },
    [refreshDay]
  );

  const saveSegment = useCallback(
    (id: string, revision: number, patch: SegmentPatch) =>
      runEdit("已修改记录", () => ipc.updateSegment(id, revision, patch)),
    [runEdit]
  );

  const backfillSegment = useCallback(
    (input: BackfillInput) => runEdit("已补录一段记录", () => ipc.backfillSegment(input)),
    [runEdit]
  );

  const deleteSegment = useCallback(
    (id: string) => runEdit("已删除这段记录", () => ipc.deleteSegment(id)),
    [runEdit]
  );

  const splitSegment = useCallback(
    (id: string, at: number) => runEdit("已拆分为两段", () => ipc.splitSegment(id, at)),
    [runEdit]
  );

  const mergeSegments = useCallback(
    (ids: string[]) => runEdit("已合并为一段", () => ipc.mergeSegments(ids)),
    [runEdit]
  );

  const saveNote = useCallback(
    (title: string, note: string) => {
      const target = pendingNote;
      if (!target) return;
      void run(async () => {
        await ipc.updateSegment(target.id, target.revision, { title, note });
        setPendingNote(await ipc.getPendingNote());
        setUndoDeadline(null);
        setUndoableLabel(null);
        refreshDay();
      });
    },
    [pendingNote, run, refreshDay]
  );

  const skipNote = useCallback(() => {
    const target=pendingNote;
    if (!target) return;
    void run(async()=>{await ipc.confirmSession(target.id);setPendingNote(await ipc.getPendingNote());});
  }, [pendingNote,run]);
  const clearError = useCallback(() => setError(null), []);

  // 撤销窗口由 undoableLabel 标记（打点或编辑均可撤销），倒计时到 0 自动失效。
  const remaining = useCountdown(undoDeadline);
  const undoRemaining = undoableLabel !== null ? remaining : 0;

  return {
    state,
    lastClosed,
    pendingNote,
    undoRemaining,
    undoableLabel,
    dayVersion,
    elapsed: useElapsed(state),
    pending,
    error,
    recovery,
    resolving,
    resolveRecovery,
    refresh,
    start,
    punch,
    pause,
    finish,
    undo,
    saveNote,
    skipNote,
    clearError,
    saveSegment,
    backfillSegment,
    deleteSegment,
    splitSegment,
    mergeSegments
  };
}
