"use client";

import { Loader } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { connectRemote, forgetRemote, restartDaemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { isAndroidApp, pairRemote, remoteHosts, scanMobilePairing } from "@/lib/remote-hosts";
import type { Connection } from "@/lib/state";
import { useApp } from "@/lib/store";
import { ConnectDialog } from "./host-switcher";
import { MobileConnect, type MobileConnectApi } from "./mobile-connect";
import { Pill } from "./pill";

type Blocking = Exclude<Connection, { status: "ready" } | { status: "reconnecting" } | { status: "offline" }>;

const COPY: Record<Exclude<Blocking["status"], "failed" | "removed">, { title: string; body: string | null }> = {
  connecting: { title: "Starting Tjommie", body: null },
  update: {
    title: "Update Tjommie on this device",
    body: "This window and Tjommie's background service are different versions. Install the latest Tjommie, then open it again.",
  },
  outside: {
    title: "Open Tjommie from the desktop app",
    body: "This page talks to Tjommie's background service, which only the desktop app can reach.",
  },
};

const PHONE: MobileConnectApi = { hosts: remoteHosts, pair: pairRemote, connect: connectRemote, forget: forgetRemote, scan: scanMobilePairing };

// Shown in place of the pages until the daemon connection is ready. In the Android app that is the phone's own start (connect to a saved PC, or pair
// with one) instead of the desktop's "open the desktop app"; a version mismatch keeps its own words everywhere, and so does a PC removing this device.
// The desktop on another PC names that PC, since it isn't its own Tjommie that is starting or failing.
export function ConnectionScreen({ connection }: { connection: Blocking }) {
  const android = useSyncExternalStore(() => () => {}, isAndroidApp, () => false);
  const host = useApp((state) => state.host);
  if (connection.status === "removed") return <RemovedScreen android={android} />;
  if (android && connection.status !== "update") return <MobileConnect status={connection.status} reason={connection.status === "failed" ? connection.reason : undefined} api={PHONE} />;
  const pc = host === null ? null : (remoteHosts().find((saved) => saved.id === host)?.name ?? "the other PC");
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 pb-11 text-center">
        {connection.status === "connecting" && <Loader className="size-5 animate-spin text-icon" aria-hidden />}
        {connection.status === "failed" ? (
          <>
            <h1 className="text-[17px] font-semibold text-title">{pc ? `Can't connect to ${pc}` : "Tjommie's background service didn't start"}</h1>
            <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{connection.reason}</p>
            <Pill primary onClick={restartDaemon}>
              Try again
            </Pill>
          </>
        ) : (
          <>
            <h1 className="text-[17px] font-semibold text-title">{pc && connection.status === "connecting" ? `Connecting to ${pc}` : COPY[connection.status].title}</h1>
            {COPY[connection.status].body && <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{COPY[connection.status].body}</p>}
          </>
        )}
      </div>
    </div>
  );
}

// The PC refused this device: it was revoked there, and nothing retries. Pair again takes a new code from that PC (a phone goes to its connect
// screen, the desktop opens Connect to another computer); Forget drops the PC from this device. The screen it replaces may have held the focus,
// so the focus starts at its heading: the removal is read out, and the keyboard goes on from there.
export function RemovedScreen({ android }: { android: boolean }) {
  const host = useApp((state) => state.host);
  const [pairing, setPairing] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), []);
  async function forget(): Promise<void> {
    if (!host) return;
    setProblem(null);
    try {
      await forgetRemote(host);
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 pb-11 text-center">
        <h1 ref={heading} tabIndex={-1} className="text-[17px] font-semibold text-title outline-none">
          This PC removed this device
        </h1>
        <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">To use it again, pair again with a new code from that PC.</p>
        {android && <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">To stop its notifications, remove this phone on that PC.</p>}
        {problem && (
          <p role="alert" className="max-w-[440px] text-[13px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <div className="flex gap-2">
          <Pill primary onClick={() => (android ? connectRemote("") : setPairing(true))}>
            Pair again
          </Pill>
          <Pill onClick={() => void forget()}>Forget</Pill>
        </div>
      </div>
      {pairing && <ConnectDialog onClose={() => setPairing(false)} />}
    </div>
  );
}
