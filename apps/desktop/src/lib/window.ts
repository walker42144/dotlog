import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
  onFullscreenChange: (cb: (fullscreen: boolean) => void): Promise<UnlistenFn | undefined> =>
    IS_TAURI
      ? listen<boolean>("tauri://fullscreen-changed", (event) => cb(event.payload)).then(
          (unlisten) => unlisten
        )
      : Promise.resolve(undefined)
};
