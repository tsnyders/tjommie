"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { LoginEntry, VaultStatus, type LoginEntry as Entry, type VaultHostAction } from "../../../protocol/src/vault";
import { createLoginsClient, LOGINS_PROBLEM } from "@/lib/logins";
import { useApp } from "@/lib/store";
import { SettingsCard, SettingsProblem, SettingsRow, SettingsSection } from "./section";

const BUTTON = "min-h-11 rounded-lg bg-pill px-3 text-sm text-text disabled:opacity-50";
export function LoginsSection() {
  const host = useApp(state => state.host), connection = useApp(state => state.connection.status);
  return <HostLogins key={`${host}:${connection}`} />;
}
function HostLogins() {
  const [client] = useState(createLoginsClient), bots = useApp(state => state.bots);
  const [status, setStatus] = useState<VaultStatus | null>(null), [entries, setEntries] = useState<Entry[]>([]), [botId, setBotId] = useState("");
  const [busy, setBusy] = useState(false), [note, setNote] = useState(""), [error, setError] = useState(false), revision = useRef(0), alive = useRef(false), reads = useRef(0);
  const refresh = useCallback(async () => {
    const expected = revision.current, read = ++reads.current;
    try {
      const [nextStatus, nextEntries] = await Promise.all([client.send({ kind: "status" }), client.send({ kind: "list" })]);
      if (alive.current && client.current() && revision.current === expected && reads.current === read) { setStatus(VaultStatus.parse(nextStatus)); setEntries(LoginEntry.array().max(1000).parse(nextEntries)); setError(false); }
    } catch { if (alive.current && revision.current === expected && reads.current === read) setError(true); }
  }, [client]);
  useEffect(() => {
    alive.current = true;
    void Promise.resolve().then(() => { if (alive.current) return refresh(); });
    const visibility = () => { if (document.visibilityState === "hidden") { revision.current++; void client.cancel(); setBusy(false); setNote("Pending host interaction cancelled."); } else void refresh(); };
    document.addEventListener("visibilitychange", visibility);
    return () => { alive.current = false; void client.cancel(); document.removeEventListener("visibilitychange", visibility); };
  }, [client, refresh]);
  async function run(action: VaultHostAction): Promise<void> {
    const expected = ++revision.current; setBusy(true); setError(false); setNote(action.kind === "lock" ? "Locking Logins…" : "Continue in Tjommie's trusted Logins window on the host.");
    try {
      const result = await client.send(action);
      if (alive.current && client.current() && expected === revision.current) { setNote(typeof result === "object" && result !== null && "status" in result && result.status === "cancelled" ? "Cancelled." : action.kind === "entry" ? "Login saved on the host." : action.kind === "delete" ? "Deletion confirmed. Any pending protected-storage cleanup is shown below." : "Logins updated."); await refresh(); }
    } catch (failure) { if (alive.current && client.current() && expected === revision.current) { setError(!(failure instanceof DOMException && failure.name === "AbortError")); setNote(failure instanceof DOMException && failure.name === "AbortError" ? "Cancelled." : ""); } }
    finally { if (alive.current && client.current() && expected === revision.current) setBusy(false); }
  }
  const cancel = () => { revision.current++; setBusy(false); setNote("Cancelled."); void client.cancel(); };
  return <SettingsSection title="Logins" note="Saved login metadata belongs to the selected host. Enter usernames and passwords only in its dedicated native Logins window; this screen never receives them.">
    <SettingsCard><SettingsRow title={status ? `Logins ${status.state}` : error ? "Logins unavailable" : "Checking Logins…"} note={status?.reason ?? "The host asks for confirmation before unlock, entry changes or filling a login."}><button type="button" className={BUTTON} disabled={busy} onClick={() => void refresh()}>Refresh</button><button type="button" className={BUTTON} disabled={busy || status?.state !== "locked"} onClick={() => void run({ kind: "unlock" })}>Unlock on host</button><button type="button" className={BUTTON} disabled={!client.current()} onClick={() => void run({ kind: "lock" })}>Lock Logins</button></SettingsRow>{!!status?.pendingDeletes && <SettingsRow title={`${status.pendingDeletes} protected-storage deletion${status.pendingDeletes === 1 ? "" : "s"} pending`} note="Cleanup is retried by the host. An entry removed from this list is not proof that its OS record was deleted." />}</SettingsCard>
    <SettingsCard><SettingsRow title="New saved login" note="Choose its bot, then enter its name, exact HTTPS origin and secret in the host's native window."><select aria-label="Bot for new login" value={botId} onChange={event => setBotId(event.target.value)} className="min-h-11 max-w-full rounded border border-divider bg-canvas px-2 text-sm"><option value="">Choose bot</option>{bots.map(bot => <option key={bot.id} value={bot.id}>{bot.name}</option>)}</select><button type="button" className={BUTTON} disabled={busy || !botId || status?.state !== "unlocked"} onClick={() => void run({ kind: "entry", botId })}>New login on host</button></SettingsRow></SettingsCard>
    <SettingsCard>{entries.length === 0 ? <SettingsRow title="No saved login metadata" note="Refresh after the host has saved an entry." /> : entries.map(entry => <SettingsRow key={entry.id} title={entry.name} note={`${entry.origin} · ${bots.find(bot => bot.id === entry.botId)?.name ?? "Missing bot"}`}><button type="button" className={BUTTON} disabled={busy || status?.state !== "unlocked"} onClick={() => void run({ kind: "entry", botId: entry.botId, id: entry.id, revision: entry.revision })}>Edit on host</button><button type="button" className={BUTTON} disabled={busy} onClick={() => void run({ kind: "delete", id: entry.id, revision: entry.revision })}>Delete on host</button></SettingsRow>)}</SettingsCard>
    {busy && <button type="button" className={`${BUTTON} mt-3`} onClick={cancel}>Cancel host interaction</button>}{note && <p role="status" className="mt-3 text-sm text-secondary">{note}</p>}{error && <SettingsProblem>{LOGINS_PROBLEM}</SettingsProblem>}
  </SettingsSection>;
}
