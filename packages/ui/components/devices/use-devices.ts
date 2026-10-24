"use client";

import { useCallback, useEffect, useState } from "react";
import type { Device, RemoteStatus } from "@tjommie/protocol";
import { daemon, request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";

// The paired devices and what the host is doing, read when the section opens and again whenever the host says its devices changed.
export function useDevices(): { devices: Device[] | null; status: RemoteStatus | null; error: string | null; reload: () => void } {
  const ready = useApp((state) => state.connection.status === "ready");
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((current) => current + 1), []);
  useEffect(() => {
    if (!ready) return;
    let live = true;
    Promise.all([request("devices.list", {}), request("remote.status", {})]).then(
      ([list, state]) => {
        if (!live) return;
        setDevices(list);
        setStatus(state);
        setError(null);
      },
      (failure: unknown) => {
        if (live) setError(errorText(failure));
      },
    );
    let off = (): void => {};
    try {
      off = daemon().subscribe((event) => {
        if (event.event === "devices.changed") reload();
      });
    } catch {
      // not connected right now: the read above reports it
    }
    return () => {
      live = false;
      off();
    };
  }, [ready, tick, reload]);
  return { devices, status, error, reload };
}
