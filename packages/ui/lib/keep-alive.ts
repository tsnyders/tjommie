import type { Connection } from "./state";

// T0 dropped the foreground service. The hidden settings branch still has this call shape, but it cannot start a native plugin.
export type KeepAlivePlugin = {
  start(options: { hostName: string }): Promise<void>;
  stop(): Promise<void>;
  status(): Promise<{ running: boolean }>;
  batteryUnrestricted(): Promise<{ value: boolean }>;
  openBatterySettings(): Promise<void>;
};

const unavailable: KeepAlivePlugin = {
  start: async () => {}, stop: async () => {}, status: async () => ({ running: false }),
  batteryUnrestricted: async () => ({ value: true }), openBatterySettings: async () => {},
};

export const keepAlivePlugin = (): KeepAlivePlugin => unavailable;
export const keepAliveEnabled = (): boolean => false;
export const setKeepAliveEnabled: (on: boolean) => void = () => undefined;
export const keepAliveRunning = (): boolean => false;
export const syncKeepAlive: (status: Connection["status"], name?: string | null) => void = () => undefined;
