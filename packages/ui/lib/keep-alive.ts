import { registerPlugin } from "@capacitor/core";
import { isAndroidApp } from "./remote-hosts";
import type { Connection } from "./state";
import { getState } from "./store";

// The frozen TjommieKeepAlive plugin (T7): an Android foreground service whose "Connected to <PC>" notification keeps the link live while the
// app is in the background.
export type KeepAlivePlugin = {
  start(options: { hostName: string }): Promise<void>;
  stop(): Promise<void>;
  status(): Promise<{ running: boolean }>;
  batteryUnrestricted(): Promise<{ value: boolean }>;
  openBatterySettings(): Promise<void>;
};

const SETTING = "tjommie.mobile.keepAlive.v1";
let plugin: KeepAlivePlugin | undefined;
let startedFor: string | null = null; // the PC the service was started for, from start() until it fails or stops
let hostName: string | null = null; // the connected PC's name, for the notification

export const keepAlivePlugin = (): KeepAlivePlugin => (plugin ??= registerPlugin<KeepAlivePlugin>("TjommieKeepAlive"));

// "Always connected", on unless turned off.
export function keepAliveEnabled(): boolean {
  return typeof localStorage === "undefined" || localStorage.getItem(SETTING) !== "false";
}

export function setKeepAliveEnabled(on: boolean): void {
  localStorage.setItem(SETTING, String(on));
  syncKeepAlive(getState().connection.status);
}

// The service is running or starting, so the link keeps pinging and reconnecting while the app is hidden. A start still pending counts:
// the phone can be put away before it answers.
export const keepAliveRunning = (): boolean => startedFor !== null;

// The service runs while a PC's link is ready and Always connected is on. It keeps running through reconnecting and offline, so the link can come
// back in the background, and stops when the PC removes this device, the PC is left or forgotten, or the setting goes off. Android lets an app start
// it only while the app is in front.
export function syncKeepAlive(status: Connection["status"], name = hostName): void {
  hostName = name;
  if (!isAndroidApp()) return;
  if (!keepAliveEnabled() || status === "removed" || status === "outside" || status === "failed") {
    if (startedFor === null) return;
    startedFor = null;
    void keepAlivePlugin().stop().catch(() => {});
    return;
  }
  if (status !== "ready" || !name || startedFor === name || (typeof document !== "undefined" && document.visibilityState === "hidden")) return;
  startedFor = name;
  keepAlivePlugin().start({ hostName: name }).catch(() => { if (startedFor === name) startedFor = null; });
}
