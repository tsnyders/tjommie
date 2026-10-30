import type { MenuItemConstructorOptions } from "electron";
import type { autoUpdater } from "electron-updater";

const shutdownFailure = "Couldn't stop the background service. The update was not installed.";

export type UpdateState =
  | { status: "idle" | "checking" | "none" }
  | { status: "downloading"; percent: number }
  | { status: "ready"; version: string }
  | { status: "error"; message: string }
  | { status: "unsupported" };

export function createUpdater(
  packaged: boolean,
  updater: typeof autoUpdater,
  env: NodeJS.ProcessEnv = process.env,
  quit: () => void = () => {},
) {
  let current: UpdateState = { status: packaged && env.TJOMMIE_UPDATES !== "off" ? "idle" : "unsupported" };
  let restartRequested = false;
  const listeners = new Set<(state: UpdateState) => void>();
  const change = (state: UpdateState, forced = false): void => {
    if (!forced && current.status === "ready" && state.status !== "ready") return;
    current = state;
    for (const listener of listeners) listener(state);
  };
  const active = current.status !== "unsupported";
  if (active) {
    updater.autoDownload = true;
    updater.autoInstallOnAppQuit = true;
    updater.logger = null;
    if (env.TJOMMIE_UPDATE_FEED) updater.setFeedURL({ provider: "generic", url: env.TJOMMIE_UPDATE_FEED });
    updater.on("checking-for-update", () => change({ status: "checking" }));
    updater.on("update-not-available", () => change({ status: "none" }));
    updater.on("download-progress", (info) => change({ status: "downloading", percent: info.percent }));
    updater.on("update-downloaded", (info) => change({ status: "ready", version: info.version }));
    updater.on("error", (error) => change({ status: "error", message: error.message }));
  }
  const check = async (): Promise<void> => {
    if (!active) return;
    try { const result = await updater.checkForUpdates(); void result?.downloadPromise?.catch(() => {}); }
    catch (error) { change({ status: "error", message: error instanceof Error ? error.message : String(error) }); }
  };
  const first = active ? setTimeout(() => void check(), 60_000) : null;
  const repeat = active ? setInterval(() => void check(), 6 * 60 * 60_000) : null;
  return {
    state: (): UpdateState => current,
    installationPending: (): boolean => restartRequested || (current.status === "ready" && updater.autoInstallOnAppQuit),
    onState: (listener: (state: UpdateState) => void): (() => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    check,
    restart: (): void => { if (current.status === "ready" && !restartRequested) { restartRequested = true; updater.autoInstallOnAppQuit = true; quit(); } },
    installOnQuit: (): void => { if (restartRequested) { restartRequested = false; updater.quitAndInstall(true, true); } },
    shutdownFailed: (): void => {
      restartRequested = false;
      updater.autoInstallOnAppQuit = false;
      change({ status: "error", message: shutdownFailure }, true);
    },
    stop: (): void => { if (first) clearTimeout(first); if (repeat) clearInterval(repeat); },
  };
}

export function trayTemplate(
  state: UpdateState,
  open: () => void,
  restart: () => void,
  quit: () => void,
): MenuItemConstructorOptions[] {
  return [
    { label: "Open Tjommie", click: open },
    { type: "separator" },
    ...(state.status === "ready" ? [{ label: `Restart to update to v${state.version}`, click: restart }] : []),
    ...(state.status === "error" ? [{ label: state.message === shutdownFailure ? "Couldn't install the update" : "Couldn't check for updates", enabled: false }] : []),
    { label: "Quit Tjommie", click: quit },
  ];
}
