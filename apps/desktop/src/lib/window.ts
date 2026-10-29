import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// 浏览器预览时窗口控制为空操作。
export const windowControls = {
  minimize: (): Promise<void> => (IS_TAURI ? getCurrentWindow().minimize() : Promise.resolve()),
  toggleMaximize: (): Promise<void> =>
    IS_TAURI ? getCurrentWindow().toggleMaximize() : Promise.resolve(),
  close: (): Promise<void> => (IS_TAURI ? getCurrentWindow().close() : Promise.resolve()),
  setFullscreen: (flag: boolean): Promise<void> =>
    IS_TAURI ? getCurrentWindow().setFullscreen(flag) : Promise.resolve(),
  isFullscreen: (): Promise<boolean> =>
    IS_TAURI ? getCurrentWindow().isFullscreen() : Promise.resolve(false),
  // Tauri 没有独立的 fullscreen 事件，全屏切换会触发 resize，借此同步状态。
  onFullscreenChange: (cb: (fullscreen: boolean) => void): Promise<UnlistenFn | undefined> =>
    IS_TAURI
      ? getCurrentWindow().onResized(() => {
          void getCurrentWindow()
            .isFullscreen()
            .then(cb).catch(() => {});
        })
      : Promise.resolve(undefined)
};
