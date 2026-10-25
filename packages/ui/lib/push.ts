import type { Client } from "@tjommie/client";
import { PushData } from "@tjommie/protocol";

type Listening = Promise<{ remove: () => Promise<void> }>;
// The phone's push as T7's wrapper exposes it: the official @capacitor/push-notifications calls, and whether this build has push at all (an APK
// built without Firebase config doesn't).
export type PushApi = {
  available(): boolean | Promise<boolean>;
  checkPermissions(): Promise<{ receive: string }>;
  requestPermissions(): Promise<{ receive: string }>; // Settings' Allow
  register(): Promise<void>;
  addListener(event: "registration", listener: (token: { value: string }) => void): Listening;
  addListener(event: "registrationError", listener: (error: { error: string }) => void): Listening;
  addListener(event: "pushNotificationActionPerformed", listener: (action: { notification: { data?: unknown } }) => void): Listening;
};

// No push until T7's wrapper (packages/mobile/src/plugins.ts) merges and is plugged in here (ruling R2). The official plugin's register() crashes
// an APK built without Firebase config, so this doesn't guess at availability.
export const nativePush: PushApi | null = null;
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

// A tapped push, once its data reads as one this app sends. A push that arrives while the app is in front shows nothing and is not listened to.
export function onPushTap(handler: (data: PushData) => void, push: PushApi | null = nativePush): () => void {
  if (!push) return () => {};
  const listening = push.addListener("pushNotificationActionPerformed", ({ notification }) => {
    const data = PushData.safeParse(notification.data);
    if (data.success) handler(data.data);
  }).catch(() => null);
  return () => { void listening.then((subscription) => subscription?.remove()).catch(() => {}); };
}
