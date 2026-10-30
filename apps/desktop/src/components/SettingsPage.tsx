import { THEMES, THEME_DETAILS } from "../lib/themes";
import { useCallback, useEffect, useRef, useState } from "react";
import { formatDuration } from "../lib/format";
import {
  friendlyMessage,
  ipc,
  toIpcError,
  type BackupEntry,
  type ExportFormat,
  type ImportPreview,
  type SettingsMap
} from "../lib/ipc";

const APP_SCOPE = "app";
const DAY_MS = 86_400_000;

const hourOptions = Array.from({ length: 24 }, (_, hour) => ({
  value: hour * 60,
  label: `${String(hour).padStart(2, "0")}:00`
}));

const secondsOptions = (values: number[]) =>
  values.map((value) => ({ value, label: `${value / 1_000} 秒` }));

const minuteOptions = (values: number[]) =>
  values.map((value) => ({ value, label: `${value} 分钟` }));

const countOptions = (values: number[]) =>
  values.map((value) => ({ value, label: `${value} 个` }));

interface OptionSelectProps {
  value: number | string;
  options: Array<{ value: number | string; label: string }>;
  onChange: (value: string) => void;
  ariaLabel: string;
}

function OptionSelect({ value, options, onChange, ariaLabel }: OptionSelectProps) {
  return (
    <select
      className="field__input setting-control"
      aria-label={ariaLabel}
      value={String(value)}
      onChange={(event) => onChange(event.target.value)}
    >
      {options.map((option) => (
        <option key={option.value} value={String(option.value)}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(0)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

const pad = (value: number) => String(value).padStart(2, "0");

function formatBackupTime(epochMs: number): string {
  const date = new Date(epochMs);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours()
  )}:${pad(date.getMinutes())}`;
}

const STATUS_LABEL: Record<ImportPreview["rows"][number]["status"], string> = {
  ok: "可导入",
  conflict: "冲突",
  invalid: "无效"
};

interface SettingsPageProps {
  onNotify: (message: string, tone?: "info" | "error") => void;
  onDataChanged: () => void;
}

export function SettingsPage({
  onNotify,
  onDataChanged
}: SettingsPageProps) {
  const [exportRange, setExportRange] = useState("all");
  const [networkPlugins, setNetworkPlugins] = useState<string[]>([]);
  const [settings, setSettings] = useState<SettingsMap | null>(null);
  const [backups, setBackups] = useState<BackupEntry[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<BackupEntry | null>(null);
  const [preview, setPreview] = useState<{ content: string; view: ImportPreview } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const loadBackups = useCallback(async () => {
    try {
      setBackups(await ipc.backupList());
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    }
  }, [onNotify]);

  useEffect(() => {
    let cancelled = false;
    ipc
      .settingsGet(APP_SCOPE)
      .then((value) => {
        if (!cancelled) setSettings(value);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setLoadError(friendlyMessage(toIpcError(reason)));
      });
    void loadBackups();
    void ipc.integrationList().then((list) => { if (!cancelled) setNetworkPlugins(list.filter((p) => p.enabled && p.capabilities.includes("network")).map((p) => p.name)); });
    return () => {
      cancelled = true;
    };
  }, [loadBackups]);

  const patch = useCallback(
    async (entries: Array<{ key: string; value: unknown }>) => {
      setBusy(entries.map((entry) => entry.key).join(","));
      try {
        setSettings(await ipc.settingsPatch(APP_SCOPE, entries));
        window.dispatchEvent(new Event("dotlog:settings-changed"));
      } catch (reason) {
        onNotify(friendlyMessage(toIpcError(reason)), "error");
      } finally {
        setBusy(null);
      }
    },
    [onNotify]
  );

  const readText = (key: string, fallback: string): string => {
    const value = settings?.[key];
    return typeof value === "string" ? value : fallback;
  };

  const readBool = (key: string, fallback: boolean): boolean => {
    const value = settings?.[key];
    return typeof value === "boolean" ? value : fallback;
  };

  const readNumber = (key: string, fallback: number): number => {
    const value = settings?.[key];
    return typeof value === "number" ? value : fallback;
  };


  const backupNow = async () => {
    setBusy("backup");
    try {
      await ipc.backupNow();
      await loadBackups();
      onNotify("已生成新的备份");
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  const restore = async (entry: BackupEntry) => {
    setBusy("restore");
    try {
      await ipc.backupRestore(entry.path);
      setRestoreTarget(null);
      onDataChanged();
      onNotify("已恢复备份并刷新状态；恢复前副本已保留，联网插件已暂停，请核对后重新启用");
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  const exportData = async (format: ExportFormat) => {
    const to = Date.now();
    const from = exportRange === "all" ? -8_640_000_000_000_000 : to - 30 * DAY_MS;
    setBusy(`export-${format}`);
    try {
      const result = await ipc.exportData(format, from, to);
      onNotify(`已导出 ${result.count} 段到 ${result.path}`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  const pickFile = () => fileRef.current?.click();

  const readFile = async (file: File) => {
    setBusy("import");
    try {
      const content = await file.text();
      const view = await ipc.importPreview(content);
      setPreview({ content, view });
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  const commitImport = async () => {
    if (!preview) return;
    setBusy("import-commit");
    try {
      const result = await ipc.importCommit(preview.content);
      setPreview(null);
      onDataChanged();
      onNotify(`已导入 ${result.inserted} 段，跳过 ${result.skipped} 段`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  const snapshotDiagnostics = async () => {
    setBusy("diagnostics");
    try {
      const result = await ipc.diagnosticsSnapshot();
      onNotify(`诊断包已保存到 ${result.path}`);
    } catch (reason) {
      onNotify(friendlyMessage(toIpcError(reason)), "error");
    } finally {
      setBusy(null);
    }
  };

  if (!settings) {
    return (
      <div className="settings-page">
        <header className="page-head">
          <h1>设置</h1>
          <p>{loadError ?? "读取中…"}</p>
        </header>
      </div>
    );
  }

  return (
    <div className="settings-page">
      <header className="page-head">
        <h1>设置</h1>
        <p>改动即时保存 · 全部数据保存在本机</p>
      </header>

      <section className="settings-group">
        <h2>记录规则</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>打点模式</label>
            <p>停止：打点后回到空闲；连续：打点同时开始下一段。</p>
          </div>
          <OptionSelect
            ariaLabel="打点模式"
            value={readText("punch.mode", "stop")}
            options={[
              { value: "stop", label: "打点即停" },
              { value: "chain", label: "连续打点" }
            ]}
            onChange={(value) => void patch([{ key: "punch.mode", value }])}
          />
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>日期边界</label>
            <p>仅影响展示与统计归属，不改原始记录。</p>
          </div>
          <OptionSelect
            ariaLabel="日期边界"
            value={readNumber("day.start_minutes", 240)}
            options={hourOptions}
            onChange={(value) =>
              void patch([{ key: "day.start_minutes", value: Number(value) }])
            }
          />
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>专注阈值</label>
            <p>整段时长达到该值的专注记录才计入专注时长。</p>
          </div>
          <OptionSelect
            ariaLabel="专注阈值"
            value={readNumber("focus.min_minutes", 10)}
            options={minuteOptions([5, 10, 15, 20, 30, 45, 60])}
            onChange={(value) =>
              void patch([{ key: "focus.min_minutes", value: Number(value) }])
            }
          />
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>撤销窗口</label>
            <p>打点后可在该时长内按 Ctrl + Z 撤销。</p>
          </div>
          <OptionSelect
            ariaLabel="撤销窗口"
            value={readNumber("punch.undo_window_ms", 15_000)}
            options={secondsOptions([5_000, 10_000, 15_000, 30_000, 60_000])}
            onChange={(value) =>
              void patch([{ key: "punch.undo_window_ms", value: Number(value) }])
            }
          />
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>短段阈值</label>
            <p>短于该时长的段会提示补记。</p>
          </div>
          <OptionSelect
            ariaLabel="短段阈值"
            value={readNumber("punch.short_segment_ms", 15_000)}
            options={secondsOptions([10_000, 15_000, 30_000, 60_000])}
            onChange={(value) =>
              void patch([{ key: "punch.short_segment_ms", value: Number(value) }])
            }
          />
        </div>
      </section>

      <section className="settings-group">
        <h2>快捷键与提醒</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>全局打点</label>
            <p>修改后需重启应用生效。</p>
          </div>
          <kbd className="settings-kbd">{readText("hotkey.punch", "Ctrl+Shift+Space")}</kbd>
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>打点提醒</label>
            <p>打点后弹出补记标题的输入框。</p>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={readBool("notify.on_punch", true)}
              onChange={(event) =>
                void patch([{ key: "notify.on_punch", value: event.target.checked }])
              }
            />
            <span className="switch__track" aria-hidden="true" />
          </label>
        </div>
      </section>

      <section className="settings-group">
        <h2>外观</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>主题</label>
            <p>跟随系统会在系统切换深浅色时同步。</p>
          </div>
          <div className="theme-picker" role="group" aria-label="主题">
            {THEMES.map(option => <button key={option.value} type="button" className="theme-choice" data-skin={option.value} aria-label={option.label}
              disabled={busy !== null} aria-pressed={readText("ui.theme", "system") === option.value}
              onClick={() => { void patch([{ key: "ui.theme", value: option.value }]); }}>
              <span className="theme-choice__swatch" style={{ background: option.color }} aria-hidden="true"><i /><i /><i /></span>
              <span className="theme-choice__copy"><strong>{option.label}</strong><small>{THEME_DETAILS[option.value]}</small></span>
            </button>)}
          </div>
        </div>
        <div className="setting-row">
          <div className="setting-row__text"><label htmlFor="motion-preference">界面动画</label><p>系统开启减少动态效果时，自动停用动画。</p></div>
          <select id="motion-preference" className="field__input setting-control" disabled={busy !== null}
            value={readText("ui.motion", "system")} onChange={event => { void patch([{ key: "ui.motion", value: event.target.value }]); }}>
            <option value="system">跟随系统</option><option value="reduced">减少动态效果</option>
          </select>
        </div>
      </section>

      <section className="settings-group">
        <h2>数据与备份</h2>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>自动备份</label>
            <p>每天使用时生成一次快照，常驻跨日也会检查，按下方策略滚动保留。</p>
          </div>
          <label className="switch">
            <input
              type="checkbox"
              checked={readBool("backup.enabled", true)}
              onChange={(event) =>
                void patch([{ key: "backup.enabled", value: event.target.checked }])
              }
            />
            <span className="switch__track" aria-hidden="true" />
          </label>
        </div>

        <div className="setting-row">
          <div className="setting-row__text">
            <label>保留策略</label>
            <p>日 / 周 / 月各保留最近若干份。</p>
          </div>
          <div className="settings-inline">
            <OptionSelect
              ariaLabel="保留日备份"
              value={readNumber("backup.keep_daily", 7)}
              options={countOptions([3, 5, 7, 10, 14])}
              onChange={(value) =>
                void patch([{ key: "backup.keep_daily", value: Number(value) }])
              }
            />
            <OptionSelect
              ariaLabel="保留周备份"
              value={readNumber("backup.keep_weekly", 4)}
              options={countOptions([2, 4, 6, 8])}
              onChange={(value) =>
                void patch([{ key: "backup.keep_weekly", value: Number(value) }])
              }
            />
            <OptionSelect
              ariaLabel="保留月备份"
              value={readNumber("backup.keep_monthly", 6)}
              options={countOptions([3, 6, 12])}
              onChange={(value) =>
                void patch([{ key: "backup.keep_monthly", value: Number(value) }])
              }
            />
          </div>
        </div>

        <div className="setting-row setting-row--stack">
          <div className="setting-row__text">
            <label>备份列表</label>
            <p>恢复会用备份覆盖当前数据库，操作前会自动校验。</p>
          </div>
          <div className="settings-actions">
            <button
              type="button"
              className="primary-btn primary-btn--sm"
              disabled={busy !== null}
              onClick={() => void backupNow()}
            >
              立即备份
            </button>
          </div>
        </div>

        <div className="plugin-manager-actions">
          <button className="ghost-btn" onClick={() => void ipc.dataOpenFolder("backups")}>打开备份目录</button>
          <button className="ghost-btn" onClick={() => void ipc.dataOpenFolder("restore-safety")}>恢复前安全副本</button>
          <button className="ghost-btn" onClick={() => void ipc.dataOpenFolder("exports")}>打开导出目录</button>
        </div>
        {backups.length === 0 ? (
          <p className="panel__empty">还没有备份文件</p>
        ) : (
          <ul className="backup-list">
            {backups.map((entry) => (
              <li key={entry.path} className="backup-row">
                <div className="backup-row__text">
                  <strong>{formatBackupTime(entry.createdAt)}</strong>
                  <span>{formatBytes(entry.sizeBytes)}</span>
                </div>
                <button
                  type="button"
                  className="ghost-btn"
                  disabled={busy !== null}
                  onClick={() => setRestoreTarget(entry)}
                >
                  恢复
                </button>
              </li>
            ))}
          </ul>
        )}

        <div className="setting-row setting-row--stack">
          <div className="setting-row__text">
            <label>导出 / 导入</label>
            <p>可导出全部记录或最近30天；导入只写入无冲突的行。完整设置恢复请使用数据库备份。</p>
          </div>
          <div className="settings-actions">
            <select className="field__input setting-control" aria-label="导出范围" value={exportRange} onChange={(e) => setExportRange(e.target.value)}><option value="all">全部记录</option><option value="30days">最近30天</option></select>
            <button
              type="button"
              className="ghost-btn"
              disabled={busy !== null}
              onClick={() => void exportData("csv")}
            >
              导出 CSV
            </button>
            <button
              type="button"
              className="ghost-btn"
              disabled={busy !== null}
              onClick={() => void exportData("json")}
            >
              导出 JSON
            </button>
            <button
              type="button"
              className="ghost-btn"
              disabled={busy !== null}
              onClick={pickFile}
            >
              导入 CSV
            </button>
            <input
              ref={fileRef}
              type="file"
              accept=".csv,text/csv"
              className="visually-hidden"
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = "";
                if (file) void readFile(file);
              }}
            />
          </div>
        </div>
      </section>

      <section className="settings-group">
        <h2>隐私</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>联网集成</label>
            <p>{networkPlugins.length ? `已启用：${networkPlugins.join("、")}。这些插件可向配置的服务发送授权内容。` : "未启用联网插件；本地记录保存在本机。侧边 Memos 也使用插件联网权限。"}</p>
          </div>
          <span className="badge">{networkPlugins.length ? "联网集成已启用" : "联网集成未启用"}</span>
        </div>
      </section>

      <section className="settings-group">
        <h2>关于</h2>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>版本</label>
            <p>数据目录：%APPDATA%\dotlog · 备份与导出在各自子目录。</p>
          </div>
          <span className="settings-version">v0.1.18</span>
        </div>
        <div className="setting-row">
          <div className="setting-row__text">
            <label>诊断包</label>
            <p>包含版本、表计数、完整性自检与最近日志，可附在问题反馈中。</p>
          </div>
          <div className="settings-actions">
            <button
              type="button"
              className="ghost-btn"
              disabled={busy !== null}
              onClick={() => void snapshotDiagnostics()}
            >
              生成诊断包
            </button>
          </div>
        </div>
      </section>

      {restoreTarget && (
        <div className="modal-backdrop" onClick={() => setRestoreTarget(null)}>
          <div className="modal" role="dialog" aria-label="恢复备份" onClick={(event) => event.stopPropagation()}>
            <div className="modal__head">
              <strong>恢复这份备份？</strong>
              <span className="modal__range">{formatBackupTime(restoreTarget.createdAt)}</span>
            </div>
            <p className="modal__hint">
              当前数据库会被备份内容覆盖，未备份的改动无法找回。建议先点「立即备份」。
            </p>
            <div className="modal__foot">
              <div className="modal__actions">
                <button type="button" className="ghost-btn" onClick={() => setRestoreTarget(null)}>
                  取消
                </button>
                <button
                  type="button"
                  className="danger-btn"
                  disabled={busy !== null}
                  onClick={() => void restore(restoreTarget)}
                >
                  确认恢复
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {preview && (
        <div className="modal-backdrop" onClick={() => setPreview(null)}>
          <div
            className="modal modal--wide"
            role="dialog"
            aria-label="导入预览"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="modal__head">
              <strong>导入预览</strong>
              <span className="modal__range">
                共 {preview.view.total} 行 · 可导入 {preview.view.importable} · 冲突{" "}
                {preview.view.conflicts} · 无效 {preview.view.invalid}
              </span>
            </div>

            <div className="import-table">
              <div className="import-table__head">
                <span>行</span>
                <span>时间</span>
                <span>标题</span>
                <span>分类</span>
                <span>状态</span>
              </div>
              {preview.view.rows.slice(0, 100).map((row) => (
                <div key={row.index} className={`import-table__row import-table__row--${row.status}`}>
                  <span>{row.index}</span>
                  <span className="import-table__time">
                    {row.status === "invalid"
                      ? "—"
                      : `${formatBackupTime(row.startAt)} · ${formatDuration(row.seconds)}`}
                  </span>
                  <span>{row.title || "未命名"}</span>
                  <span>{row.categoryName ?? "未分类"}</span>
                  <span title={row.reason ?? undefined}>{STATUS_LABEL[row.status]}</span>
                </div>
              ))}
            </div>

            <div className="modal__foot">
              <p className="modal__hint">冲突与无效行会被跳过，其余行按时间写入。</p>
              <div className="modal__actions">
                <button type="button" className="ghost-btn" onClick={() => setPreview(null)}>
                  取消
                </button>
                <button
                  type="button"
                  className="primary-btn primary-btn--sm"
                  disabled={busy !== null || preview.view.importable === 0}
                  onClick={() => void commitImport()}
                >
                  导入 {preview.view.importable} 行
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
