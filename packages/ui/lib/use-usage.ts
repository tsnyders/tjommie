"use client";

import { useEffect, useRef, useState } from "react";
import type { UsageReport } from "@tjommie/protocol";
import { daemon, request } from "./connection";
import { errorText } from "./labels";
import { useApp } from "./store";
import { localZone, matching, throttled, untilUsageDay, usageDayKey, usageQuery, type RangeKind, type UsageFilters } from "./usage";

const REFETCH_MS = 1000; // usage events come in bursts while a bot works

export type UsageSpec = { kind: RangeKind; filters: UsageFilters };

// A usage report for a range and filters, read when it first shows, whenever the spec changes, and (once a second at most, however steady
// the events) when the daemon says usage changed for the account or bot it covers. The range ends at the moment of each read, so a refetch sees the
// newest turns. `null` means nothing to ask for. A refresh of the same spec keeps its last report if it fails; another spec never shows it.
export function useUsage(spec: UsageSpec | null): { report: UsageReport | null; error: string | null } {
  const ready = useApp((state) => state.connection.status === "ready");
  const [clock, setClock] = useState(() => Date.now());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const tick = (): void => {
      setClock(Date.now());
      timer = setTimeout(tick, Math.min(60_000, untilUsageDay(new Date(), localZone())));
    };
    const resume = (): void => { if (document.visibilityState === "visible") setClock(Date.now()); };
    tick();
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => { clearTimeout(timer); window.removeEventListener("focus", resume); document.removeEventListener("visibilitychange", resume); };
  }, []);
  // What was read is kept with the spec it answered, and shown only while that is still the spec asked: a new range or filter never shows
  // the last one's totals, even if its own read fails.
  const [held, setHeld] = useState<{ key: string; report: UsageReport } | null>(null);
  const [failed, setFailed] = useState<{ key: string; message: string } | null>(null);
  const text = spec ? JSON.stringify({ spec, day: usageDayKey(new Date(clock), localZone()) }) : null;
  const latest = useRef(spec);
  useEffect(() => {
    latest.current = spec;
  });
  useEffect(() => {
    if (!ready || text === null) return;
    let live = true;
    let generation = 0;
    const read = (): void => {
      const asked = latest.current;
      if (!asked) return;
      const now = new Date(), zone = localZone();
      if (JSON.stringify({ spec: asked, day: usageDayKey(now, zone) }) !== text) { setClock(now.getTime()); return; }
      const reading = ++generation;
      request("usage.query", usageQuery(asked.kind, asked.filters, now, zone)).then(
        (next) => {
          if (!live || generation !== reading) return;
          setHeld({ key: text, report: next });
          setFailed(null);
        },
        (failure: unknown) => {
          if (live && generation === reading) setFailed({ key: text, message: errorText(failure) });
        },
      );
    };
    const refresh = throttled(read, REFETCH_MS);
    read();
    let off = (): void => {};
    try {
      off = daemon().subscribe((event) => {
        if (event.event !== "usage.changed") return;
        const filters = latest.current?.filters;
        if (filters?.account && filters.account !== event.data.accountId) return;
        if (filters?.bot && filters.bot !== event.data.botId) return;
        refresh.poke();
      });
    } catch {
      // not connected right now: the read above reports it, and the next connection reads again
    }
    return () => {
      live = false;
      refresh.cancel();
      off();
    };
  }, [ready, text]);
  return { report: matching(held && { key: held.key, value: held.report }, text), error: matching(failed && { key: failed.key, value: failed.message }, text) };
}
