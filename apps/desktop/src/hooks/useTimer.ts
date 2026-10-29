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

const UNDO_WINDOW_MS = 15_000;
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
  const [now, setNow] = useState(0);
  const startAt = state?.active?.startAt ?? null;

  useEffect(() => {
    if (startAt === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [startAt]);

  if (startAt === null) return 0;
  if (now === 0) return state?.elapsedSeconds ?? 0;
  return Math.max(0, Math.floor((now - startAt) / 1_000));
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
    setPending(true);
    try {
      await action();
      setError(null);
    } catch (cause) {
      setError(toIpcError(cause));
    } finally {
      setPending(false);
    }
  }, []);

  const refreshDay = useCallback(() => setDayVersion((value) => value + 1), []);

  // 启动恢复提示：后端在 setup 阶段已完成检测并缓存结果。
  useEffect(() => {
    let cancelled = false;
    void ipc
      .recoveryGet()
      .then((info) => {
        if (!cancelled) setRecovery(info.required ? info : null);
      })
      .catch(() => {
        // 恢复提示不是核心路径，读取失败时静默降级。
      });
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
          setPendingNote(null);
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
      setPendingNote(result.pendingNote ? result.closedSegment : null);
      setUndoDeadline(result.undoToken ? Date.now() + UNDO_WINDOW_MS : null);
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
    void run(() => close(() => ipc.pause()));
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
      const offPunch = await listen<string>(QUICK_PUNCH_EVENT, () => {
        const current = stateRef.current;
        if (current?.phase === "recording" && !current.paused) {
          punch();
        } else {
          start();
        }
      });
      if (disposed) {
        offPunch();
        return;
      }
      unlisteners.push(offPunch);

      const offPause = await listen(QUICK_PAUSE_EVENT, () => {
        const current = stateRef.current;
        if (current?.phase === "recording" && !current.paused) {
          pause();
        }
      });
      if (disposed) {
        offPause();
        return;
      }
      unlisteners.push(offPause);
    };

    void register().catch(() => {
      // 事件系统不可用时静默降级，界面按钮仍可操作。
    });

    return () => {
      disposed = true;
      for (const off of unlisteners) off();
    };
  }, [punch, pause, start]);

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
      setPending(true);
      try {
        const result = await action();
        setLastClosed(null);
        setPendingNote(null);
        setUndoableLabel(label);
        setUndoDeadline(Date.now() + UNDO_WINDOW_MS);
        setState(await ipc.getState());
        refreshDay();
        setError(null);
        return result;
      } catch (cause) {
        throw toIpcError(cause);
      } finally {
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
        setPendingNote(null);
        setUndoDeadline(null);
        setUndoableLabel(null);
        refreshDay();
      });
    },
    [pendingNote, run, refreshDay]
  );

  const skipNote = useCallback(() => setPendingNote(null), []);
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
    refresh: refreshDay,
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
