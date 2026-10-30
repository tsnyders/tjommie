"use client";

import { useEffect, useState } from "react";
import { Loader, ScanLine } from "lucide-react";
import { pairingProblem, pairingReady } from "@/components/devices/pairing";
import { errorText } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { FIELD, Label } from "./controls";
import { Pill } from "./pill";

export type MobileHostView = { id: string; name: string };
// What this screen needs from the app: the saved hosts, pairing, connecting, forgetting one, and the phone's QR scanner. The real ones are
// lib/remote-hosts.ts and lib/connection.ts; the screen itself knows nothing of keys or sockets.
export type MobileConnectApi = {
  hosts(): MobileHostView[];
  pair(code: string, name: string): Promise<MobileHostView>;
  connect(id?: string): void;
  forget(id: string): Promise<void>;
  scan?(): Promise<string | null>;
};

export const DEFAULT_PHONE_NAME = "Android phone";

// The phone scans the PC's code; another PC pastes it.
const DEVICE = {
  phone: {
    name: DEFAULT_PHONE_NAME,
    label: "Name for this phone",
    about: "Tjommie runs on your PC. This phone connects to it on the same Wi-Fi or, through your PC's relay, from anywhere. Your PC has to be awake with Tjommie open.",
    steps: "On your PC open Settings, then Devices, then Pair a device. Scan its code, or paste it here.",
  },
  pc: {
    name: "Another PC",
    label: "Name for this PC",
    about: "Use Tjommie on another PC from this one. It connects on the same Wi-Fi or, through that PC's relay, from anywhere. That PC has to be awake with Tjommie open.",
    steps: "On the other PC open Settings, then Devices, then Pair a device. Copy its code and paste it here.",
  },
} as const;

// Connect to a PC: one this device was paired with, or a new one by its pairing code. It is where the phone starts, and the desktop's "Connect to
// another computer" (a dialog, whose title heads it). Nothing here says the device is connected: that is the connection state, shown by the app once
// it is. A code is read from the box once and the box is cleared before pairing starts, so it never lingers.
export function MobileConnect({ status, reason, api, device = "phone", dialog = false }: {
  status: "outside" | "failed" | "connecting";
  reason?: string;
  api: MobileConnectApi;
  device?: keyof typeof DEVICE;
  dialog?: boolean;
}) {
  const words = DEVICE[device];
  const [hosts, setHosts] = useState(() => api.hosts());
  const [code, setCode] = useState("");
  const [name, setName] = useState<string>(words.name);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  const wrong = pairingProblem(code, now);
  const connecting = status === "connecting";

  async function pair(): Promise<void> {
    if (busy || !pairingReady(code, Date.now()) || name.trim() === "") return;
    const typed = code;
    setCode(""); // before anything is awaited: the code is not kept
    setBusy(true);
    setProblem(null);
    try {
      const host = await api.pair(typed.trim(), name.trim());
      setHosts(api.hosts());
      api.connect(host.id);
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  async function scan(): Promise<void> {
    setProblem(null);
    try {
      const text = (await api.scan?.()) ?? null;
      if (text !== null) setCode(text); // it only fills the box: pairing starts when the person presses Connect
    } catch (error) {
      setProblem(`${errorText(error)} Paste the code instead.`);
    }
  }

  async function forget(id: string): Promise<void> {
    setProblem(null);
    try {
      await api.forget(id);
      setHosts(api.hosts());
    } catch (error) {
      setProblem(errorText(error));
    }
  }

  const choices = (
    <>
      {hosts.length > 0 && (
        <ul aria-label="Saved PCs" className="flex flex-col gap-2">
          {hosts.map((host) => (
            <li key={host.id} className={cn("flex items-center gap-2 rounded-[12px] p-3", dialog ? "border border-divider" : "bg-card")}>
              <span className="min-w-0 flex-1 truncate text-[14px] text-text">{host.name}</span>
              <Pill primary disabled={connecting} onClick={() => api.connect(host.id)} aria-label={`Connect to ${host.name}`}>
                Connect
              </Pill>
              <Pill disabled={connecting} onClick={() => void forget(host.id)} aria-label={`Forget ${host.name}`}>
                Forget
              </Pill>
            </li>
          ))}
        </ul>
      )}
      {device === "phone" && hosts.length > 0 && <p className="text-[12px] leading-[18px] text-secondary">To stop its notifications, remove this phone on that PC.</p>}
      <section aria-label="Pair a new PC" className={cn("flex flex-col gap-3", !dialog && "rounded-[12px] bg-card p-4")}>
        <h2 className="text-[14px] font-semibold text-title">{hosts.length > 0 ? "Pair another PC" : "Pair"}</h2>
        <p className="text-[12px] leading-[18px] text-secondary">{words.steps}</p>
        <div>
          <Label htmlFor="pair-code">Pairing code</Label>
          <textarea
            id="pair-code"
            value={code}
            onChange={(event) => setCode(event.target.value)}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            rows={3}
            aria-invalid={wrong !== null}
            className={`${FIELD} mt-1.5 resize-none py-2 font-mono text-[12px] [overflow-wrap:anywhere]`}
          />
          {wrong && (
            <p role="alert" className="mt-1.5 text-[12px] leading-[18px] text-danger">
              {wrong}
            </p>
          )}
        </div>
        <div>
          <Label htmlFor="pair-name">{words.label}</Label>
          <input id="pair-name" value={name} maxLength={80} onChange={(event) => setName(event.target.value)} className={`${FIELD} mt-1.5 h-10`} />
        </div>
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <div className="flex items-center justify-end gap-2">
          {device === "phone" && (
            <Pill disabled={busy || connecting} onClick={() => void scan()}>
              <ScanLine className="size-4" aria-hidden />
              Scan code
            </Pill>
          )}
          <Pill primary disabled={busy || connecting || !pairingReady(code, now) || name.trim() === ""} onClick={() => void pair()}>
            {busy ? "Pairing" : "Connect"}
          </Pill>
        </div>
      </section>
    </>
  );

  if (dialog) {
    return (
      <div className="mt-3 flex flex-col gap-4">
        <p className="text-[13px] leading-[18px] text-body">{words.about}</p>
        {choices}
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[440px] flex-1 flex-col justify-center gap-5 px-6 py-10">
        <div className="text-center">
          {connecting && <Loader className="mx-auto mb-3 size-5 animate-spin text-icon" aria-hidden />}
          <h1 className="text-[17px] font-semibold text-title">{connecting ? "Connecting to your PC" : hosts.length > 0 ? "Connect to your PC" : "Pair with your PC"}</h1>
          <p className="mt-2 text-[13px] leading-[18px] text-secondary">{status === "failed" && reason ? reason : words.about}</p>
        </div>
        {choices}
      </div>
    </div>
  );
}
