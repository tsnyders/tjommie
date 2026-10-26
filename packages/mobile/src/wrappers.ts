// The three native plugins as TypeScript sees them. These are the generic building blocks only: no pairing, no host list and no connection logic here,
// which belong to the layer above once the transport is settled. Everything crosses the bridge as strings; binary is base64.

import type { PushNotificationsPlugin } from "@capacitor/push-notifications";

export type SecureStorePlugin = {
  // Seals bytes (base64) under a name with an AES-256-GCM key that lives in the Android Keystore and never leaves it. Replaces what was there.
  seal(options: { name: string; valueBase64: string }): Promise<void>;
  // The bytes (base64) sealed under the name, or null if there are none. A value that fails its check rejects with code "corrupt".
  read(options: { name: string }): Promise<{ valueBase64: string | null }>;
  has(options: { name: string }): Promise<{ present: boolean }>;
  delete(options: { name: string }): Promise<void>;
};

export type SocketClosedReason = "local" | "remote" | "error" | "protocol";
export type SocketFrameEvent = { connectionId: string; dataBase64: string };
export type SocketClosedEvent = { connectionId: string; reason: SocketClosedReason; message?: string };
export type SocketNetworkEvent = { type: "wifi" | "cellular" | "none" | "other" };

export type SocketPlugin = {
  // Opens a TCP connection. The host is an address or name; a wildcard, multicast or empty one is refused. `timeoutMs` bounds the connect (default 1000, 200 to 10000).
  open(options: { host: string; port: number; timeoutMs?: number }): Promise<{ connectionId: string }>;
  // Sends one Noise frame (1 to 65,535 bytes) with a 4-byte big-endian length. Rejects when the write backlog is full.
  send(options: { connectionId: string; dataBase64: string }): Promise<void>;
  close(options: { connectionId: string }): Promise<void>; // idempotent
  addListener(event: "frame", listener: (event: SocketFrameEvent) => void): Promise<{ remove: () => Promise<void> }>;
  addListener(event: "closed", listener: (event: SocketClosedEvent) => void): Promise<{ remove: () => Promise<void> }>;
  addListener(event: "network", listener: (event: SocketNetworkEvent) => void): Promise<{ remove: () => Promise<void> }>;
};

export type PushRoutingPlugin = { available(): Promise<{ value: boolean }>; unregister(): Promise<void> };

export function createPush(routing: PushRoutingPlugin, official: PushNotificationsPlugin) {
  return {
    available: async () => (await routing.available()).value,
    checkPermissions: () => official.checkPermissions(),
    requestPermissions: () => official.requestPermissions(),
    register: async () => {
      if (!(await routing.available()).value) throw new Error("Push isn't in this build.");
      return official.register();
    },
    unregister: () => routing.unregister(),
    addListener: official.addListener.bind(official),
  };
}

export type PairingScannerPlugin = {
  // Opens Google's code scanner for a QR code. The text of what it read, or null when the person backed out. Rejects with code "unavailable" when the
  // scanner can't run on this phone (the caller then offers pasting the code). The text is never logged.
  scan(): Promise<{ text: string | null }>;
};

export const PLUGIN_NAMES = { secureStore: "TjommieSecureStore", socket: "TjommieSocket", scanner: "TjommiePairingScanner" } as const;

type Register = <T>(name: string) => T;

export function createPlugins(register: Register): { secureStore: SecureStorePlugin; socket: SocketPlugin; scanner: PairingScannerPlugin } {
  return {
    secureStore: register<SecureStorePlugin>(PLUGIN_NAMES.secureStore),
    socket: register<SocketPlugin>(PLUGIN_NAMES.socket),
    scanner: register<PairingScannerPlugin>(PLUGIN_NAMES.scanner),
  };
}

// Bytes and base64, for the plugin's string-only bridge.
export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let at = 0; at < bytes.length; at += 0x8000) binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  return btoa(binary);
}

export function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let at = 0; at < binary.length; at += 1) bytes[at] = binary.charCodeAt(at);
  return bytes;
}

export const MAX_FRAME_BYTES = 65535;
