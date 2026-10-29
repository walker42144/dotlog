import { useEffect, useRef, useState } from "react";
import { formatDuration, formatTime } from "../lib/format";
import type { SegmentView } from "../lib/ipc";

interface NotePopoverProps {
  segment: SegmentView;
  saving: boolean;
  onSave: (title: string, note: string) => void;
  onSkip: () => void;
}

/** 补记浮层：Enter 保存、Shift+Enter 换行、Esc 跳过。 */
export function NotePopover({ segment, saving, onSave, onSkip }: NotePopoverProps) {
  const [title, setTitle] = useState(segment.title);
  const [note, setNote] = useState(segment.note ?? "");
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

  const save = () => {
    if (saving) return;
    onSave(title, note);
  };

  const endAt = segment.endAt ?? segment.startAt + segment.durationSeconds * 1_000;

  return (
    <>
      <div className="note-backdrop" aria-hidden="true" onClick={onSkip} />
      <div className="note-popover" role="dialog" aria-modal="false" aria-label="补记这一段">
        <header className="note-popover__head">
          <div>
            <strong>补记这一段</strong>
            <span className="note-popover__range">
              {formatTime(segment.startAt)} – {formatTime(endAt)} ·{" "}
              {formatDuration(segment.durationSeconds)}
            </span>
          </div>
          <button className="note-popover__close" type="button" aria-label="跳过补记" onClick={onSkip}>
            ×
          </button>
        </header>

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
              if (event.key === "Escape") {
                event.preventDefault();
                onSkip();
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
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                save();
              }
              if (event.key === "Escape") {
                event.preventDefault();
                onSkip();
              }
            }}
          />
        </label>

        <footer className="note-popover__foot">
          <span className="note-popover__hint">Enter 保存 · Esc 跳过</span>
          <div className="note-popover__actions">
            <button className="ghost-btn" type="button" disabled={saving} onClick={onSkip}>
              跳过
            </button>
            <button
              className="primary-btn primary-btn--sm"
              type="button"
              disabled={saving}
              aria-busy={saving}
              onClick={save}
            >
              保存补记
            </button>
          </div>
        </footer>
      </div>
    </>
  );
}
