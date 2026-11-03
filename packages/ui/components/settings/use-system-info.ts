"use client";

import { useEffect, useState } from "react";
import type { Result } from "@tjommie/protocol";
import type { SpeechStatus } from "../../../protocol/src/speech";
import { request } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { localSpeech } from "@/lib/local-speech";
import { getState, useApp } from "@/lib/store";

type Info = {
  hostKey: string | null;
  connection: string;
  revision: number;
  host: Result<"diagnostics.get"> | null;
  device: { appVersion: string; platform: "windows" | "macos" | "linux" } | null;
  speech: SpeechStatus | null;
  hostUnavailable: boolean;
};

export function useSystemInfo() {
  const host = useApp((state) => state.host);
  const connection = useApp((state) => state.connection.status);
  const [revision, setRevision] = useState(0);
  const [info, setInfo] = useState<Info | null>(null);
  useEffect(() => {
    let cancelled = false;
    const read = async (): Promise<void> => {
      const [hostInfo, device, speech] = await Promise.allSettled([
        connection === "ready" ? request("diagnostics.get", {}) : Promise.resolve(null),
        desktop()?.diagnostics?.info() ?? Promise.resolve(null),
        localSpeech()?.status() ?? Promise.resolve(null),
      ]);
      if (cancelled || getState().host !== host) return;
      setInfo({ hostKey: host, connection, revision, host: hostInfo.status === "fulfilled" ? hostInfo.value : null, device: device.status === "fulfilled" ? device.value : null, speech: speech.status === "fulfilled" ? speech.value : null, hostUnavailable: hostInfo.status === "rejected" });
    };
    void read();
    return () => { cancelled = true; };
  }, [host, connection, revision]);
  const current = info?.hostKey === host && info.connection === connection && info.revision === revision ? info : null;
  return { info: current, loading: current === null, refresh: () => setRevision((value) => value + 1) };
}
