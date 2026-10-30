import { useEffect, useRef, useState } from "react";
import { formatDuration, formatTime, fromLocalInput, toLocalInput } from "../lib/format";
import {
  ipc,
  friendlyMessage,
  type CategoryView,
  toIpcError,
  type BackfillInput,
  type ConflictView,
  type IpcError,
  type SegmentPatch,
  type SegmentView
} from "../lib/ipc";

export type EditorTarget =
  | { mode: "edit"; segment: SegmentView; nextSegment: SegmentView | null }
  | { mode: "backfill"; startAt: number; endAt: number };

interface SegmentEditorProps {
  target: EditorTarget;
  pending: boolean;
  onClose: () => void;
  onSave: (id: string, revision: number, patch: SegmentPatch) => Promise<SegmentView>;
  onBackfill: (input: BackfillInput) => Promise<SegmentView>;
  onDelete: (id: string) => Promise<void>;
  onSplit: (id: string, at: number) => Promise<SegmentView[]>;
  onMerge: (ids: string[]) => Promise<SegmentView>;
}

/** 冲突时段里尽量保留较长的一侧，把 [start, end) 裁剪到空档。 */
function clipToGap(
  start: number,
  end: number,
  conflicts: ConflictView[]
): { start: number; end: number } | null {
  let clippedStart = start;
  let clippedEnd = end;
  const sorted = [...conflicts].sort((left, right) => left.startAt - right.startAt);
  for (const conflict of sorted) {
    const conflictEnd = conflict.endAt ?? Date.now();
    if (conflictEnd <= clippedStart || conflict.startAt >= clippedEnd) continue;
    const leftLength = conflict.startAt - clippedStart;
    const rightLength = clippedEnd - conflictEnd;
    if (leftLength >= rightLength) {
      clippedEnd = conflict.startAt;
    } else {
      clippedStart = conflictEnd;
    }
  }
  return clippedEnd > clippedStart ? { start: clippedStart, end: clippedEnd } : null;
}

/** 编辑错误优先用服务端中文文案（E_INVALID_RANGE 在全局字典里是打点语境）。 */
function editorMessage(error: IpcError): string {
  if (error.code === "E_INVALID_RANGE" && error.message) return error.message;
  return friendlyMessage(error);
}

/** 段编辑 / 补录对话框：Esc 关闭，错误内联展示，冲突给出裁剪选项。 */
export function SegmentEditor({
  target,
  pending,
  onClose,
  onSave,
  onBackfill,
  onDelete,
  onSplit,
  onMerge
}: SegmentEditorProps) {
  const editing = target.mode === "edit" ? target.segment : null;
  const nextSegment = target.mode === "edit" ? target.nextSegment : null;
  const initialStart = target.mode === "edit" ? target.segment.startAt : target.startAt;
  const initialEnd = target.mode === "edit" ? target.segment.endAt : target.endAt;

  const [categories, setCategories] = useState<CategoryView[]>([]);
  const [categoryId, setCategoryId] = useState(editing?.category?.id ?? "");
  const [focus, setFocus] = useState(editing?.isFocus ?? false);
  const [ignored, setIgnored] = useState(editing?.isIgnored ?? false);
  const [tags, setTags] = useState(editing?.tags.map((t) => t.name).join(", ") ?? "");
  useEffect(() => { void ipc.categories().then(setCategories).catch(() => setCategories([])); }, []);
  const [title, setTitle] = useState(editing?.title ?? "");
  const [note, setNote] = useState(editing?.note ?? "");
  const [startInput, setStartInput] = useState(() => toLocalInput(initialStart));
  const [endInput, setEndInput] = useState(() => (initialEnd ? toLocalInput(initialEnd) : ""));
  const [error, setError] = useState<IpcError | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [splitOpen, setSplitOpen] = useState(false);
  const [splitInput, setSplitInput] = useState(() =>
    toLocalInput(initialStart + Math.floor(((initialEnd ?? Date.now()) - initialStart) / 2))
  );
  const titleRef = useRef<HTMLInputElement>(null);
  const previousFocus = useRef<Element | null>(document.activeElement);

  useEffect(() => {
    titleRef.current?.focus();
    return () => {
      if (previousFocus.current instanceof HTMLElement) {
        previousFocus.current.focus();
      }
    };
  }, []);

  const active = editing?.status === "active";
  const conflicts = error?.detail?.conflicts ?? [];

  const runAction = async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
      onClose();
    } catch (cause) {
      setError(toIpcError(cause));
    }
  };

  const save = () => {
    if (pending) return;
    const startMs = fromLocalInput(startInput);
    const endMs = active ? null : fromLocalInput(endInput);
    if (startMs === null || (!active && endMs === null)) {
      setError({ code: "E_INVALID_RANGE", message: "请填写完整的起止时间" });
      return;
    }
    if (endMs !== null && endMs <= startMs) {
      setError({ code: "E_INVALID_RANGE", message: "结束时间必须晚于开始时间" });
      return;
    }

    if (target.mode === "backfill") {
      void runAction(() =>
        onBackfill({ startAt: startMs, endAt: endMs as number, title, note, categoryId: categoryId || null, isFocus: focus, isIgnored: ignored })
      );
      return;
    }

    const segment = target.segment;
    const patch: SegmentPatch = {};
    if (categoryId !== (segment.category?.id ?? "")) patch.categoryId = categoryId || null;
    if (focus !== segment.isFocus) patch.isFocus = focus;
    if (ignored !== segment.isIgnored) patch.isIgnored = ignored;
    const parsedTags = tags.split(/[,，]/).map((s) => s.trim().replace(/^#/, "")).filter(Boolean);
    if (parsedTags.join(",") !== segment.tags.map((t) => t.name).join(",")) patch.tags = parsedTags;
    if (title.trim() !== segment.title) patch.title = title;
    const nextNote = note.trim();
    if (nextNote !== (segment.note ?? "")) patch.note = note;
    // datetime-local 只有分钟精度：与原值同分钟就不算作改动，避免无谓的 revision 递增。
    if (Math.floor(startMs / 60_000) !== Math.floor(segment.startAt / 60_000)) {
      patch.startAt = startMs;
    }
    if (
      !active &&
      endMs !== null &&
      Math.floor(endMs / 60_000) !== Math.floor((segment.endAt ?? 0) / 60_000)
    ) {
      patch.endAt = endMs;
    }
    if (Object.keys(patch).length === 0) {
      onClose();
      return;
    }
    void runAction(() => onSave(segment.id, segment.revision, patch));
  };

  const applyClip = () => {
    const startMs = fromLocalInput(startInput);
    const endMs = fromLocalInput(endInput);
    if (startMs === null || endMs === null || conflicts.length === 0) return;
    const clipped = clipToGap(startMs, endMs, conflicts);
    if (!clipped) {
      setError({ code: "E_OVERLAP", message: "这段时间已被完全占满，请换一个区间" });
      return;
    }
    setStartInput(toLocalInput(clipped.start));
    setEndInput(toLocalInput(clipped.end));
    setError(null);
  };

  const confirmSplit = () => {
    if (!editing) return;
    const at = fromLocalInput(splitInput);
    if (at === null) {
      setError({ code: "E_INVALID_RANGE", message: "请选择拆分时间点" });
      return;
    }
    void runAction(() => onSplit(editing.id, at));
  };

  const headerRange =
    target.mode === "edit"
      ? `${formatTime(target.segment.startAt)} – ${
          target.segment.endAt ? formatTime(target.segment.endAt) : "现在"
        } · ${formatDuration(target.segment.durationSeconds)}`
      : `${formatTime(target.startAt)} – ${formatTime(target.endAt)}`;

  return (
    <>
      <div className="modal-backdrop" aria-hidden="true" onClick={onClose} />
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={editing ? "编辑这段记录" : "补录一段记录"}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          }
        }}
      >
        <header className="modal__head">
          <div>
            <strong>{editing ? "编辑这段记录" : "补录一段记录"}</strong>
            <span className="modal__range">{headerRange}</span>
          </div>
          <button className="modal__close" type="button" aria-label="关闭" onClick={onClose}>
            ×
          </button>
        </header>

        {error && (
          <div className="modal__error" role="alert">
            <span>{editorMessage(error)}</span>
            {conflicts.length > 0 && (
              <>
                <ul className="modal__conflicts">
                  {conflicts.map((conflict) => (
                    <li key={conflict.id}>
                      {conflict.title || "未命名"} · {formatTime(conflict.startAt)} –{" "}
                      {conflict.endAt ? formatTime(conflict.endAt) : "现在"}
                    </li>
                  ))}
                </ul>
                <button className="link-btn" type="button" onClick={applyClip}>
                  裁剪到空档
                </button>
              </>
            )}
          </div>
        )}

        <label className="field">
          <span className="field__label">标题</span>
          <input
            ref={titleRef}
            className="field__input"
            type="text"
            value={title}
            maxLength={80}
            placeholder="这一段做了什么？"
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                save();
              }
            }}
          />
        </label>

        <label className="field">
          <span className="field__label">备注</span>
          <textarea
            className="field__input field__input--area"
            rows={2}
            value={note}
            maxLength={500}
            placeholder="补充细节（可选）"
            onChange={(event) => setNote(event.target.value)}
          />
        </label>

        <div className="field-row">
          <label className="field"><span className="field__label">分类</span><select className="field__input" value={categoryId} onChange={(e) => setCategoryId(e.target.value)}><option value="">未分类</option>{categories.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}</select></label>
          {editing && <label className="field"><span className="field__label">标签（逗号分隔）</span><input className="field__input" value={tags} onChange={(e) => setTags(e.target.value)} maxLength={1200} /></label>}
        </div>
        <div className="plugin-manager-actions"><label><input type="checkbox" checked={focus} onChange={(e) => setFocus(e.target.checked)} /> 专注记录</label><label><input type="checkbox" checked={ignored} onChange={(e) => setIgnored(e.target.checked)} /> 不计入有效时长</label></div>
        <div className="field-row">
          <label className="field">
            <span className="field__label">开始时间</span>
            <input
              className="field__input"
              type="datetime-local"
              value={startInput}
              onChange={(event) => setStartInput(event.target.value)}
            />
          </label>
          <label className="field">
            <span className="field__label">结束时间</span>
            <input
              className="field__input"
              type="datetime-local"
              value={endInput}
              disabled={active}
              title={active ? "进行中的记录由打点结束" : undefined}
              onChange={(event) => setEndInput(event.target.value)}
            />
          </label>
        </div>
        {active && <p className="modal__hint">进行中的记录由打点结束，这里只能修改开始时间。</p>}

        {editing && !confirmingDelete && (
          <div className="modal__tools">
            <button
              className="ghost-btn"
              type="button"
              disabled={pending}
              onClick={() => setSplitOpen((open) => !open)}
            >
              拆分
            </button>
            {nextSegment && !active && (
              <button
                className="ghost-btn"
                type="button"
                disabled={pending}
                title={`与「${nextSegment.title || "未命名"}」合并为一段`}
                onClick={() => void runAction(() => onMerge([editing.id, nextSegment.id]))}
              >
                并入下一段
              </button>
            )}
          </div>
        )}

        {editing && splitOpen && !confirmingDelete && (
          <div className="modal__split">
            <input
              className="field__input"
              type="datetime-local"
              aria-label="拆分时间点"
              value={splitInput}
              onChange={(event) => setSplitInput(event.target.value)}
            />
            <button
              className="ghost-btn"
              type="button"
              disabled={pending}
              onClick={confirmSplit}
            >
              在此拆分
            </button>
          </div>
        )}

        <footer className="modal__foot">
          {confirmingDelete ? (
            <>
              <span className="modal__hint">确定删除这段记录？可在设置的撤销时限内恢复。</span>
              <div className="modal__actions">
                <button
                  className="ghost-btn"
                  type="button"
                  disabled={pending}
                  onClick={() => setConfirmingDelete(false)}
                >
                  取消
                </button>
                <button
                  className="danger-btn"
                  type="button"
                  disabled={pending}
                  aria-busy={pending}
                  onClick={() => editing && void runAction(() => onDelete(editing.id))}
                >
                  确认删除
                </button>
              </div>
            </>
          ) : (
            <>
              {editing ? (
                <button
                  className="danger-btn danger-btn--plain"
                  type="button"
                  disabled={pending}
                  onClick={() => setConfirmingDelete(true)}
                >
                  删除
                </button>
              ) : (
                <span className="modal__hint">补录不能超过 24 小时</span>
              )}
              <div className="modal__actions">
                <button className="ghost-btn" type="button" disabled={pending} onClick={onClose}>
                  取消
                </button>
                <button
                  className="primary-btn primary-btn--sm"
                  type="button"
                  disabled={pending}
                  aria-busy={pending}
                  onClick={save}
                >
                  保存
                </button>
              </div>
            </>
          )}
        </footer>
      </div>
    </>
  );
}
