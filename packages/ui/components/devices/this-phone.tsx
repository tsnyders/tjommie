"use client";

import { useEffect, useState, type ReactNode } from "react";
import { request } from "@/lib/connection";
import { keepAliveEnabled, keepAlivePlugin, setKeepAliveEnabled } from "@/lib/keep-alive";
import { nativePush, registerPushIfAllowed, type PushApi } from "@/lib/push";
import { Switch } from "../controls";
import { HostSwitcher } from "../host-switcher";
import { Pill } from "../pill";

// "Always connected" and the battery row wait on the Android background spike (T0): false hides both if the owner drops the keep-alive service.
const ALWAYS_CONNECTED = true;

export type Notifications = "allow" | "on" | "off" | "unavailable";
const NOTIFICATIONS: Record<Exclude<Notifications, "allow">, string> = { on: "On", off: "Off", unavailable: "Not in this build" };
const answered = (receive: string): Notifications => (receive === "granted" ? "on" : receive === "denied" ? "off" : "allow");

// Where this phone's notifications stand: not in this build (no push, or an APK built without Firebase), on, refused, or not asked yet.
export async function notificationState(push: PushApi | null): Promise<Notifications> {
  if (!push || !(await push.available())) return "unavailable";
  return answered((await push.checkPermissions()).receive);
}

// Allow: Android asks once. On yes the PC gets this phone's push token now, not only at the next connection.
export async function allowNotifications(push: PushApi | null): Promise<Notifications> {
  if (!push || !(await push.available())) return "unavailable";
  const state = answered((await push.requestPermissions()).receive);
  if (state === "on") await registerPushIfAllowed({ call: request }, push).catch(() => {}); // not connected: the next connection sends it
  return state;
}

// Settings → Devices in the Android app: this phone's own settings. The battery and notification states are read again on return from Android's
// settings.
export function ThisPhone() {
  const [restricted, setRestricted] = useState(false);
  const [notifications, setNotifications] = useState<Notifications | null>(null);
  useEffect(() => {
    const read = (): void => {
      if (ALWAYS_CONNECTED) void keepAlivePlugin().batteryUnrestricted().then(({ value }) => setRestricted(!value), () => setRestricted(false));
      void notificationState(nativePush).then(setNotifications, () => setNotifications("unavailable"));
    };
    read();
    document.addEventListener("visibilitychange", read);
    return () => document.removeEventListener("visibilitychange", read);
  }, []);
  return <PhoneSettings restricted={restricted} notifications={notifications} onNotifications={setNotifications} />;
}

export function PhoneSettings({ restricted, notifications, onNotifications }: { restricted: boolean; notifications: Notifications | null; onNotifications: (state: Notifications) => void }) {
  const [alwaysOn, setAlwaysOn] = useState(keepAliveEnabled);
  return (
    <>
      <div className="mt-3">
        <HostSwitcher phone />
      </div>
      <h3 className="mt-9 text-[13px] font-semibold leading-[18px] text-label">This phone</h3>
      <section className="mt-2.5 flex max-w-[700px] flex-col divide-y divide-divider rounded-[12px] bg-card px-4">
        {ALWAYS_CONNECTED && (
          <Row title="Always connected" note="Keeps a notification showing so Tjommie stays connected in the background. Uses a little more battery.">
            <Switch
              label="Always connected"
              checked={alwaysOn}
              onChange={(next) => {
                setKeepAliveEnabled(next);
                setAlwaysOn(next);
              }}
            />
          </Row>
        )}
        {ALWAYS_CONNECTED && restricted && (
          <Row title="Let Tjommie run in the background" note="On Samsung: Settings → Battery → Background usage limits → Never sleeping apps → add Tjommie.">
            <Pill onClick={() => void keepAlivePlugin().openBatterySettings().catch(() => {})}>Battery settings</Pill>
          </Row>
        )}
        <Row title="Notifications">
          {notifications === "allow" ? (
            <Pill onClick={() => void allowNotifications(nativePush).then(onNotifications, () => {})}>Allow</Pill>
          ) : (
            notifications && <span className="shrink-0 text-[13px] text-secondary">{NOTIFICATIONS[notifications]}</span>
          )}
        </Row>
      </section>
    </>
  );
}

function Row({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <div className="flex min-h-14 items-center justify-between gap-4 py-3">
      <div>
        <p className="text-[14px] text-text">{title}</p>
        {note && <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">{note}</p>}
      </div>
      {children}
    </div>
  );
}
