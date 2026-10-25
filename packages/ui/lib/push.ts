import type { Client } from "@tjommie/client";
import { PushData } from "@tjommie/protocol";
import { Capacitor } from "@capacitor/core";
import { push } from "../../mobile/src/plugins.ts";

type Listening = Promise<{ remove: () => Promise<void> }>;
// The phone's push as T7's wrapper exposes it: the official @capacitor/push-notifications calls, and whether this build has push at all (an APK
// built without Firebase config doesn't).
export type PushApi = {
  available(): boolean | Promise<boolean>;
  checkPermissions(): Promise<{ receive: string }>;
  requestPermissions(): Promise<{ receive: string }>; // Settings' Allow
  register(): Promise<void>;
  unregister(): Promise<void>;
  addListener(event: "registration", listener: (token: { value: string }) => void): Listening;
  addListener(event: "registrationError", listener: (error: { error: string }) => void): Listening;
  addListener(event: "pushNotificationActionPerformed", listener: (action: { notification: { data?: unknown } }) => void): Listening;
};

export const nativePush: PushApi | null = Capacitor.isNativePlatform() && Capacitor.getPlatform() === "android" ? push : null;
let token: Promise<string> | undefined; // this phone's push token, asked for once per run

function pushToken(push: PushApi): Promise<string> {
  token ??= new Promise<string>((resolve, reject) => {
    void (async () => {
      await push.addListener("registration", ({ value }) => resolve(value));
      await push.addListener("registrationError", ({ error }) => reject(new Error(error)));
      await push.register();
    })().catch(reject);
  }).catch((error: unknown) => {
    token = undefined;
    throw error;
  });
  return token;
}

// Called once a PC has welcomed this phone: gives the PC this phone's push token, when this build has push and the person allowed notifications.
// It never asks for the permission itself; Settings does.
export async function registerPushIfAllowed(client: Pick<Client, "call">, push: PushApi | null = nativePush): Promise<void> {
  if (!push || !(await push.available()) || (await push.checkPermissions()).receive !== "granted") return;
  await client.call("device.registerPush", { token: await pushToken(push) });
}

export async function unregisterPush(push: PushApi | null = nativePush): Promise<void> {
  token = undefined;
  if (push && await push.available()) await push.unregister();
}

// A tapped push, once its data reads as one this app sends. A push that arrives while the app is in front shows nothing and is not listened to.
export function onPushTap(handler: (data: PushData) => void, push: PushApi | null = nativePush): () => void {
  if (!push) return () => {};
  const listening = push.addListener("pushNotificationActionPerformed", ({ notification }) => {
    // Android's tap intent adds FCM transport keys beside the four app fields.
    const raw = notification.data;
    if (!raw || typeof raw !== "object") return;
    const fields = raw as Record<string, unknown>;
    if (Object.keys(fields).some((key) => !["v", "hostId", "kind", "botId", "from", "collapse_key", "message_type"].includes(key) && !key.startsWith("google.") && !key.startsWith("gcm."))) return;
    const data = PushData.safeParse({ v: fields.v, hostId: fields.hostId, kind: fields.kind, botId: fields.botId });
    if (data.success) handler(data.data);
  }).catch(() => null);
  return () => { void listening.then((subscription) => subscription?.remove()).catch(() => {}); };
}
