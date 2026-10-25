import { registerPlugin } from "@capacitor/core";
import { PushNotifications } from "@capacitor/push-notifications";
import { createPlugins, createPush, type PushRoutingPlugin } from "./wrappers.ts";

// Capacitor plugin proxies are safe to import on web and desktop; callers gate native operations by platform.
export const { secureStore, socket, scanner } = createPlugins((name) => registerPlugin(name));
export const push = createPush(registerPlugin<PushRoutingPlugin>("TjommiePushRouting"), PushNotifications);

export type AppearanceInsets = { imeBottom: number; keyboardVisible: boolean };
export const appearance = registerPlugin<{
  setCanvas(options: { color: string; dark: boolean }): Promise<void>;
  getInsets(): Promise<AppearanceInsets>;
}>("TjommieAppearance");
