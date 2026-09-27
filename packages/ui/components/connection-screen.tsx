"use client";

import { Loader } from "lucide-react";
import { restartDaemon } from "@/lib/connection";
import type { Connection } from "@/lib/state";
import { Pill } from "./pill";

type Blocking = Exclude<Connection, { status: "ready" } | { status: "reconnecting" }>;

const COPY: Record<Exclude<Blocking["status"], "failed">, { title: string; body: string | null }> = {
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

// Shown in place of the pages until the daemon connection is ready.
export function ConnectionScreen({ connection }: { connection: Blocking }) {
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 pb-11 text-center">
        {connection.status === "connecting" && <Loader className="size-5 animate-spin text-icon" aria-hidden />}
        {connection.status === "failed" ? (
          <>
            <h1 className="text-[17px] font-semibold text-title">Tjommie&apos;s background service didn&apos;t start</h1>
            <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{connection.reason}</p>
            <Pill primary onClick={restartDaemon}>
              Try again
            </Pill>
          </>
        ) : (
          <>
            <h1 className="text-[17px] font-semibold text-title">{COPY[connection.status].title}</h1>
            {COPY[connection.status].body && <p className="max-w-[440px] text-[13px] leading-[18px] text-secondary">{COPY[connection.status].body}</p>}
          </>
        )}
      </div>
    </div>
  );
}
