import type { Client } from "@tjommie/client";
import { PushData } from "@tjommie/protocol";
import { Capacitor } from "@capacitor/core";
import { push } from "../../mobile/src/plugins.ts";
import { atMost } from "./utils";

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
const PUSH_WAIT_MS = 5_000;
type Target = Pick<Client, "call">;
type Desired = { push: PushApi; target: Target | null; wanted: boolean; available?: boolean; allowed?: boolean; failed?: boolean };
let desired: Desired | null = null;
let live: string | null | undefined; // undefined until native state is known, including during a mutation
let sent: { to: Target; token: string } | null = null;
let nativePending: Promise<void> | null = null;
let reconciling: Promise<void> | null = null;
let again = false;
const tokenListeners = new WeakSet<PushApi>();
const errorListeners = new WeakSet<PushApi>();
let requested = false; // register resolves before its event; suppress duplicate calls, without associating events with calls

// A timeout releases the loop, never the native slot. Only this completion schedules its next pass.
async function nativeCall<T>(current: Desired, work: () => T | Promise<T>, done: (value: T) => void = () => {}): Promise<void> {
  nativePending = Promise.resolve().then(work).then(done, () => { current.failed = true; }).finally(() => {
    nativePending = null;
    void reconcile();
  });
  await atMost(PUSH_WAIT_MS, nativePending);
}

async function pass(): Promise<void> {
  const current = desired;
  if (!current || current.failed || nativePending || (current.wanted && !current.target)) return;
  const { push, target, wanted } = current;
  // Each pass captures the target before its await; stale checks cannot update the next welcome's state.
  if (current.available === undefined) {
    await nativeCall(current, () => push.available(), (value) => { if (desired === current) current.available = value; });
  } else if (!current.available) {
    return;
  } else if (wanted && current.allowed === undefined) {
    await nativeCall(current, () => push.checkPermissions(), ({ receive }) => { if (desired === current) current.allowed = receive === "granted"; });
  } else if (!tokenListeners.has(push)) {
    // Capacitor replays retained events on attachment; listen before the first mutation and never remove these listeners.
    await nativeCall(current, () => push.addListener("registration", ({ value }) => {
      if (desired?.push !== push) return;
      live = value;
      requested = false;
      desired.failed = false;
      void reconcile();
    }), () => { tokenListeners.add(push); });
  } else if (!errorListeners.has(push)) {
    await nativeCall(current, () => push.addListener("registrationError", () => {
      if (desired?.push !== push) return;
      if (requested && desired.wanted && typeof live !== "string") desired.failed = true;
      requested = false;
      void reconcile();
    }), () => { errorListeners.add(push); });
  } else if (!wanted && live !== null) {
    live = undefined;
    requested = false;
    await nativeCall(current, () => push.unregister(), () => { live = null; });
  } else if (wanted && current.allowed && typeof live !== "string" && !requested) {
    live = undefined;
    requested = true;
    await nativeCall(current, () => push.register().catch((error: unknown) => { requested = false; throw error; }));
  } else if (wanted && current.allowed && target && typeof live === "string" && (sent?.to !== target || sent.token !== live)) {
    const given = { to: target, token: live };
    sent = given;
    await atMost(PUSH_WAIT_MS, target.call("device.registerPush", { token: given.token }).catch(() => {
      if (sent === given) sent = null;
      current.failed = true; // a later welcome can retry; a failed link must not spin
    }));
  }
}

// Coalesced: a change while a pass runs makes it go round again, and every caller waits until the passes are done.
function reconcile(): Promise<void> {
  if (reconciling) {
    again = true;
    return reconciling;
  }
  reconciling = (async () => {
    try {
      do {
        again = false;
        await pass();
      } while (again);
    } finally {
      reconciling = null;
    }
  })();
  return reconciling;
}

// Welcome owns the target synchronously. Settings can recheck permission for that target without supplying a moving client wrapper.
export async function registerPushIfAllowed(client: Target | null = desired?.target ?? null, push: PushApi | null = nativePush): Promise<void> {
  if (!push || !client) return;
  desired = { push, target: client, wanted: true };
  await reconcile();
}

// The last saved PC was forgotten: the phone unregisters, unless a PC welcomes it again first.
export async function unregisterPush(push: PushApi | null = nativePush): Promise<void> {
  desired = push ? { push, target: null, wanted: false } : null;
  sent = null;
  await reconcile();
}

// The PC the window shows is being forgotten, or left: nothing more of this phone's push goes to that connection.
export function stopSendingPush(): void {
  if (desired) desired = { ...desired, target: null };
  sent = null;
  void reconcile();
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
