"use client";

import Link from "next/link";
import { Menu } from "@base-ui/react/menu";
import { useCallback, useEffect, useRef, useState } from "react";
import { Check, ChevronDown, KeyRound } from "lucide-react";
import type { BrowserState } from "@tjommie/protocol";
import { LoginEntry, VaultStatus, type VaultFillTarget, type VaultHostAction } from "../../../protocol/src/vault";
import { Row } from "@/components/bot-form/rows";
import { MENU_ITEM, MENU_POPUP } from "@/components/menu";
import { Pill } from "@/components/pill";
import { createLoginsClient, LOGINS_PROBLEM } from "@/lib/logins";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

export function browserLoginTarget(state: BrowserState): VaultFillTarget | null {
  if (state.control !== "human" || state.canControl !== true || state.privateActive || !state.activePageId || state.controlEpoch === undefined) return null;
  return { botId: state.botId, generation: state.generation, pageId: state.activePageId, controlEpoch: state.controlEpoch };
}
export function BrowserLogins({ state, onRefresh }: { state: BrowserState; onRefresh: () => void }) {
  const host = useApp(value => value.host), connection = useApp(value => value.connection.status);
  return <BrowserLoginScope key={`${host}:${connection}:${state.botId}:${state.generation}`} state={state} onRefresh={onRefresh} />;
}
function BrowserLoginScope({ state, onRefresh }: { state: BrowserState; onRefresh: () => void }) {
  const [client] = useState(createLoginsClient), [open, setOpen] = useState(false), [status, setStatus] = useState<VaultStatus | null>(null), [entries, setEntries] = useState<LoginEntry[]>([]), [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false), [notice, setNotice] = useState(""), [error, setError] = useState(false), revision = useRef(0), alive = useRef(false), reads = useRef(0);
  const refresh = useCallback(async () => {
    const expected = revision.current, read = ++reads.current;
    try {
      const [info, list] = await Promise.all([client.send({ kind: "status" }), client.send({ kind: "list", botId: state.botId })]);
      if (alive.current && expected === revision.current && client.current() && read === reads.current) { setStatus(VaultStatus.parse(info)); setEntries(LoginEntry.array().max(1000).parse(list)); setError(false); }
    } catch { if (alive.current && expected === revision.current && read === reads.current) setError(true); }
  }, [client, state.botId]);
  useEffect(() => { if (open && !state.privateActive) void Promise.resolve().then(() => { if (alive.current) return refresh(); }); }, [open, state.privateActive, refresh]);
  useEffect(() => {
    alive.current = true;
    const visibility = () => { if (document.visibilityState === "hidden") { revision.current++; void client.cancel(); setBusy(false); setNotice("Pending host interaction cancelled."); } };
    document.addEventListener("visibilitychange", visibility);
    return () => { alive.current = false; void client.cancel(); document.removeEventListener("visibilitychange", visibility); };
  }, [client]);
  const target = browserLoginTarget(state), tab = state.tabs.find(value => value.id === state.activePageId);
  let origin = ""; try { const url = new URL(tab?.url ?? ""); if (url.protocol === "https:" && !url.username && !url.password) origin = url.origin; } catch { /* Blank and non-HTTPS pages cannot receive a saved login. */ }
  const choices = entries.filter(entry => entry.botId === state.botId && entry.origin === origin), entry = choices.find(value => value.id === selected);
  async function run(action: VaultHostAction): Promise<void> {
    const expected = ++revision.current; setBusy(true); setError(false); setNotice("Confirm in Tjommie's trusted Logins window on the host.");
    try {
      const result = await client.send(action);
      if (alive.current && expected === revision.current && client.current()) { const cancelled = typeof result === "object" && result !== null && "status" in result && result.status === "cancelled"; setNotice(cancelled ? "Cancelled." : action.kind === "fill" ? "Login filled privately. Automation stays paused." : action.kind === "finish" ? "Private fields cleared or browser closed. Resume the bot separately when ready." : "Logins updated."); onRefresh(); if (action.kind === "unlock") await refresh(); }
    } catch (failure) { if (alive.current && expected === revision.current && client.current()) { setError(!(failure instanceof DOMException && failure.name === "AbortError")); setNotice(""); } }
    finally { if (alive.current && expected === revision.current && client.current()) setBusy(false); }
  }
  const cancel = () => { revision.current++; setBusy(false); setNotice("Cancelled."); void client.cancel(); };
  return <div className="min-w-0 text-[14px] leading-[21px]">
    {state.privateActive ? <div className="flex flex-wrap items-center gap-2 rounded-[12px] bg-card p-4">
      <p className="basis-full text-[13px] leading-[18px] text-secondary">Private interaction. Remote fields and pictures are hidden. Clearing fields or closing this browser requires host confirmation and keeps automation paused.</p>
      <Pill className="h-auto! min-h-8 max-w-full whitespace-normal! py-2 max-sm:min-h-11" disabled={busy || state.privateEpoch === undefined} onClick={() => { if (state.privateEpoch !== undefined) void run({ kind: "finish", botId: state.botId, generation: state.generation, privateEpoch: state.privateEpoch, mode: "clear" }); }}>Clear private fields on host</Pill>
      <Pill className="h-auto! min-h-8 max-w-full whitespace-normal! py-2 max-sm:min-h-11" disabled={busy || state.privateEpoch === undefined} onClick={() => { if (state.privateEpoch !== undefined) void run({ kind: "finish", botId: state.botId, generation: state.generation, privateEpoch: state.privateEpoch, mode: "close" }); }}>Close private browser on host</Pill>
    </div> : <Row icon={KeyRound} title="Saved logins" open={open} onToggle={() => { if (open) cancel(); setOpen(!open); }}>
      <div className="flex min-w-0 flex-wrap items-center gap-2 pb-2">
        <p className="basis-full text-[12px] leading-[18px] text-secondary [overflow-wrap:anywhere]">Only metadata is shown here. The host confirms its exact HTTPS destination before filling. {status ? `Logins ${status.state}.` : error ? "Logins unavailable." : "Checking host…"} {status?.reason}</p>
        <Menu.Root>
          <Menu.Trigger aria-label="Saved login for this origin" render={<Pill className="max-w-full max-sm:min-h-11" />}>
            <span className="min-w-0 truncate">{entry?.name ?? `Choose login for ${origin || "an HTTPS page"}`}</span><ChevronDown className="size-4 shrink-0" aria-hidden />
          </Menu.Trigger>
          <Menu.Portal><Menu.Positioner sideOffset={6} align="start" className="z-50"><Menu.Popup className={cn(MENU_POPUP, "max-h-[var(--available-height)] max-w-[calc(100vw-32px)] overflow-y-auto")}>
            <Menu.RadioGroup value={entry?.id ?? ""} onValueChange={(value: unknown) => { if (typeof value === "string") setSelected(value); }}>
              <Menu.RadioItem value="" closeOnClick className={MENU_ITEM}><span className="min-w-0 flex-1 truncate">Choose login</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>
              {choices.map(value => <Menu.RadioItem key={value.id} value={value.id} closeOnClick className={MENU_ITEM}><span className="min-w-0 flex-1 truncate">{value.name}</span><Menu.RadioItemIndicator><Check aria-hidden /></Menu.RadioItemIndicator></Menu.RadioItem>)}
            </Menu.RadioGroup>
          </Menu.Popup></Menu.Positioner></Menu.Portal>
        </Menu.Root>
        <Pill className="max-sm:min-h-11" disabled={busy || !target || !entry || status?.state !== "unlocked"} onClick={() => { if (target && entry) void run({ kind: "fill", ...target, id: entry.id, revision: entry.revision }); }}>Fill on host</Pill>
        <Pill className="max-sm:min-h-11" disabled={busy || status?.state !== "locked"} onClick={() => void run({ kind: "unlock" })}>Unlock on host</Pill>
        <Link href="/settings/?section=logins" className="inline-flex items-center text-[13px] text-accent hover:underline max-sm:min-h-11">Manage Logins</Link>
        {!target && <p className="basis-full text-[12px] leading-[18px] text-secondary">Take control of the current browser page before filling a login.</p>}
      </div>
    </Row>}
    {busy && <Pill className="mt-2 max-sm:min-h-11" onClick={cancel}>Cancel host interaction</Pill>}{notice && <p role="status" className="mt-2 text-[12px] leading-[18px] text-secondary">{notice}</p>}{error && <p role="alert" className="mt-2 text-[12px] leading-[18px] text-danger">{LOGINS_PROBLEM}</p>}
  </div>;
}
