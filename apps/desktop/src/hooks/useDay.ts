import { useEffect, useState } from "react";
import { ipc, toIpcError, type DayView, type IpcError } from "../lib/ipc";

export interface DayController {
  day: DayView | null;
  loading: boolean;
  error: IpcError | null;
}

/** 今日日视图；`version` 变化时重新拉取（命令成功或撤销后）。 */
export function useDay(version: number): DayController {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const refresh = () => setTick((v) => v + 1);
    const id = window.setInterval(refresh, 15_000);
    window.addEventListener("focus", refresh);
    return () => { window.clearInterval(id); window.removeEventListener("focus", refresh); };
  }, []);
  const [day, setDay] = useState<DayView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<IpcError | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void ipc
      .dayToday()
      .then((next) => {
        if (cancelled) return;
        setDay(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(toIpcError(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [version, tick]);

  return { day, loading, error };
}
