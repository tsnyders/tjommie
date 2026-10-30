"use client";

import { useState } from "react";
import { DEFAULT_RELAY_URL, RelayUrl, type RemoteStatus } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { FIELD, Label } from "../controls";
import { Pill } from "../pill";

const STATE: Record<RemoteStatus["relay"]["state"], string> = { online: "Connected", connecting: "Connecting", error: "Can't reach the relay", off: "Off" };

// Saves the relay's address: an https one, or none to turn the relay off. Returns why it couldn't, or null.
export async function saveRelayUrl(typed: string): Promise<string | null> {
  const trimmed = typed.trim();
  const url = trimmed === "" ? null : RelayUrl.safeParse(trimmed);
  if (url && !url.success) return "Use an https:// address.";
  try {
    await request("settings.set", { relayUrl: url ? url.data : null });
    return null;
  } catch (error) {
    return errorText(error);
  }
}

// Settings → Devices: the relay this PC reaches its devices through from anywhere, and how it's doing as the PC last said (nothing until it has).
// Devices pinned this PC's key when they paired, so a different relay never changes who they trust. Push is known once the relay is connected.
export function RelayRow({ saved, relay, publicUrl = DEFAULT_RELAY_URL }: { saved: string | null; relay: RemoteStatus["relay"] | null; publicUrl?: string | null }) {
  const [draft, setDraft] = useState(saved ?? "");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function save(typed: string): Promise<void> {
    setBusy(true);
    setProblem(null);
    setProblem(await saveRelayUrl(typed));
    setBusy(false);
  }
  return (
    <section className="mt-6 max-w-[760px] rounded-[12px] bg-card p-4">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 className="text-[15px] font-semibold leading-[20px] text-title">Relay</h3>
        {relay && (
          <p className="text-[12px] leading-[18px] text-secondary">
            {STATE[relay.state]}
            {relay.state === "online" && (relay.push ? " · Push: available" : " · Push: not set up on this relay")}
          </p>
        )}
      </div>
      <form
        className="mt-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!busy) void save(draft);
        }}
      >
        <Label htmlFor="relay-url">Address</Label>
        <div className="mt-1.5 flex gap-2">
          <input
            id="relay-url"
            type="url"
            inputMode="url"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            placeholder="https://"
            autoComplete="off"
            spellCheck={false}
            className={`${FIELD} h-10 min-w-0 flex-1`}
          />
          <Pill primary type="submit" disabled={busy || draft.trim() === (saved ?? "")}>
            Save
          </Pill>
        </div>
      </form>
      {problem && (
        <p role="alert" className="mt-2 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      {publicUrl !== null && saved !== publicUrl && (
        <Pill className="mt-3" disabled={busy} onClick={() => void save(publicUrl)}>
          Use the public relay
        </Pill>
      )}
    </section>
  );
}
