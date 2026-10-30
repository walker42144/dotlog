import { useEffect, useState } from "react";
import { windowControls } from "../lib/window";
import {
  CloseIcon,
  ExitFullscreenIcon,
  FullscreenIcon,
  MaximizeIcon,
  MinimizeIcon,
  MoonIcon,
  SunIcon
} from "./icons";

interface TitleBarProps {
  dateLabel: string;
  statusLabel: string;
  statusTone: "idle" | "recording" | "paused";
  theme: "light" | "dark";
  onToggleTheme: () => void;
}

export function TitleBar({
  dateLabel,
  statusLabel,
  statusTone,
  theme,
  onToggleTheme
}: TitleBarProps) {
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    windowControls.isFullscreen().then((v) => { if (!disposed) setFullscreen(v); }).catch(() => {});
    windowControls.onFullscreenChange((value) => setFullscreen(value)).then((fn) => {
      if (disposed) fn?.(); else unlisten = fn;
    }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // 不乐观更新：调用失败时图标保持原样，成功时由 resize 回调同步真实状态。
  const toggleFullscreen = () => {
    void windowControls.setFullscreen(!fullscreen).catch(() => {});
  };

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        <span className="brand-dot" />
        <span className="brand-name">打点</span>
        <span className="brand-sub">DotLog</span>
      </div>

      <div className="titlebar__center" data-tauri-drag-region>
        <div className="date-nav">
          <span className="date-nav__label">{dateLabel}</span>

        </div>
      </div>

      <div className="titlebar__right" data-tauri-drag-region>
        <div className="status-pill" role="status" aria-live="polite">
          <span className={`status-dot status-dot--${statusTone}`} />
          <span>{statusLabel}</span>
        </div>
        <button
          className="win-btn"
          type="button"
          onClick={onToggleTheme}
          title={theme === "dark" ? "切换到浅色主题" : "切换到深色主题"}
          aria-label={theme === "dark" ? "切换到浅色主题" : "切换到深色主题"}
        >
          {theme === "dark" ? <SunIcon size={16} /> : <MoonIcon size={16} />}
        </button>
        <div className="titlebar__divider" />
        <div className="win-controls">
          <button
            className="win-btn"
            type="button"
            onClick={() => void windowControls.minimize()}
            title="最小化"
            aria-label="最小化窗口"
          >
            <MinimizeIcon />
          </button>
          <button
            className="win-btn"
            type="button"
            onClick={() => void windowControls.toggleMaximize()}
            title="最大化 / 还原"
            aria-label="最大化或还原窗口"
          >
            <MaximizeIcon />
          </button>
          <button
            className="win-btn"
            type="button"
            onClick={toggleFullscreen}
            title={fullscreen ? "退出全屏" : "全屏"}
            aria-label={fullscreen ? "退出全屏" : "全屏显示"}
          >
            {fullscreen ? <ExitFullscreenIcon /> : <FullscreenIcon />}
          </button>
          <button
            className="win-btn win-btn--close"
            type="button"
            onClick={() => void windowControls.close()}
            title="关闭"
            aria-label="关闭窗口"
          >
            <CloseIcon />
          </button>
        </div>
      </div>
    </header>
  );
}
