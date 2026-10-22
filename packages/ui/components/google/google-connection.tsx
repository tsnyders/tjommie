"use client";

import { useEffect, useRef, useState } from "react";
import { Mail } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { attemptProblem, abilities, canClearLocal, cardLine, canRead, googleStep, needsReconsent, serviceList, validClientId, watchGoogle, type GoogleApi, type GoogleStatus } from "@/lib/google-connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { ConfirmDialog, Dialog } from "../dialog";
import { FIELD, Label } from "../controls";
import { Pill } from "../pill";

// The Google card in the bot panel (WF1 Phase 1): one Google account for the app, and a per-bot choice to let this bot
// read its Gmail. Connecting happens once, in the system browser; after that Google work goes through Google's API.
export function GoogleCard({ bot, api }: { bot: Bot; api: GoogleApi }) {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<GoogleStatus | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // A new connection (the daemon restarted) is a new client, so the status is read and followed again from it.
  const ready = useApp((state) => state.connection.status === "ready");
  useEffect(() => (ready ? watchGoogle(api, setStatus, (error) => setProblem(errorText(error))) : undefined), [api, ready]);
  const choose = (next: boolean): void => {
    setOpen(next);
    if (next) api.status().then(setStatus, () => {}); // fresh when the dialog opens
  };
  return (
    <section className="mt-4 flex items-center gap-3 rounded-[12px] bg-card p-3">
      <Mail className="size-5 shrink-0 text-icon" aria-hidden />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[13px] font-semibold leading-[18px] text-title">Google</h3>
          <button type="button" aria-label="Manage Google" onClick={() => choose(true)} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            {status && googleStep(status) === "connected" ? "Manage" : "Connect"}
          </button>
        </div>
        <p className="mt-0.5 break-words text-[12px] leading-[18px] text-secondary">{cardLine(status, bot.name, bot.id)}</p>
        {problem && !status && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
      <GoogleDialog bot={bot} api={api} status={status} onStatus={setStatus} open={open} onOpenChange={choose} />
    </section>
  );
}

function GoogleDialog({ bot, api, status, onStatus, open, onOpenChange }: { bot: Bot; api: GoogleApi; status: GoogleStatus | null; onStatus: (status: GoogleStatus) => void; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<"disconnect" | "clear" | null>(null);
  const attempt = useRef<string | null>(null); // the sign-in this window began, which Cancel ends
  const run = async (action: () => Promise<GoogleStatus | void>): Promise<void> => {
    setBusy(true);
    setProblem(null);
    try {
      const next = await action();
      if (next) onStatus(next);
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange} title="Google" width={420} closable>
        {status ? (
          <GoogleBody
            bot={bot}
            status={status}
            busy={busy}
            problem={problem}
            onConfigure={(id, secret) => void run(() => api.configure(id, secret))}
            onConnect={() =>
              void run(async () => {
                attempt.current = (await api.connect()).attempt; // opens Google in the system browser; the status then follows it
                return api.status();
              })
            }
            onCancel={() =>
              void run(async () => {
                const id = status.attempt?.id ?? attempt.current;
                if (!id) return;
                await api.cancel(id);
                return api.status();
              })
            }
            onGrant={(allowed) => void run(() => api.grant(bot.id, allowed))}
            onDisconnect={() => setConfirming("disconnect")}
            onClear={() => setConfirming("clear")}
          />
        ) : (
          <p className="mt-3 text-[13px] leading-[18px] text-secondary">Checking Google…</p>
        )}
      </Dialog>
      <ConfirmDialog
        open={confirming !== null}
        onOpenChange={(next) => !next && setConfirming(null)}
        title={confirming === "clear" ? "Clear local Google settings?" : "Disconnect Google?"}
        body={
          confirming === "clear"
            ? "This removes the Google settings Tjommie saved on this computer, so you can set it up again. Google may still list Tjommie under your account's permissions; remove it there to revoke it fully."
            : "Every bot loses its Google access, and you'll need to connect again to give it back."
        }
        confirm={confirming === "clear" ? "Clear settings" : "Disconnect"}
        danger
        onConfirm={() => {
          setConfirming(null);
          void run(() => api.disconnect());
        }}
      />
    </>
  );
}

const copyFor = (services: GoogleStatus["services"]): string =>
  `Tjommie connects to your ${serviceList(services)} through Google's API. A bot you allow can ${abilities(services)}, as you. Only allow bots you trust. This doesn't sign the built-in browser in to Google.`;

// What the dialog shows for the status: set up the app's Google client, connect, wait, or manage.
export function GoogleBody({
  bot,
  status,
  busy,
  problem,
  onConfigure,
  onConnect,
  onCancel,
  onGrant,
  onDisconnect,
  onClear,
}: {
  bot: Bot;
  status: GoogleStatus;
  busy: boolean;
  problem: string | null;
  onConfigure: (clientId: string, clientSecret: string) => void;
  onConnect: () => void;
  onCancel: () => void;
  onGrant: (allowed: boolean) => void;
  onDisconnect: () => void;
  onClear: () => void;
}) {
  const step = googleStep(status);
  const shown = problem ?? attemptProblem(status);
  return (
    <div className="mt-3 flex flex-col gap-4 text-[13px] leading-[18px] text-body">
      <p>{copyFor(status.services)}</p>
      {step === "setup" && <ClientForm busy={busy} onSave={onConfigure} />}
      {step === "connect" && (
        <div className="flex flex-col gap-3">
          <p>Connect once. Google opens in your own browser, where you choose the account and say yes to full access to Gmail, Drive and Calendar.</p>
          <div className="flex justify-end">
            <Pill primary disabled={busy} onClick={onConnect}>
              Connect Google
            </Pill>
          </div>
        </div>
      )}
      {step === "waiting" && (
        <div className="flex flex-col gap-3">
          <p role="status">Finish signing in in your browser. This updates by itself when you&apos;re done.</p>
          <div className="flex justify-end">
            <Pill disabled={busy} onClick={onCancel}>
              Cancel
            </Pill>
          </div>
        </div>
      )}
      {step === "connected" && (
        <div className="flex flex-col gap-3">
          <p>
            Connected as <span className="font-medium text-text">{status.email ?? "your Google account"}</span>.
          </p>
          {needsReconsent(status) && (
            <div className="flex flex-col gap-2 rounded-[10px] bg-pill p-3">
              <p>This connection was made with read-only access. Connect again to let bots send, edit and delete; until then no bot can.</p>
              <div className="flex justify-end">
                <Pill primary disabled={busy} onClick={onConnect}>
                  Reconnect Google
                </Pill>
              </div>
            </div>
          )}
          <label className="flex items-center gap-2.5 text-text">
            <input
              type="checkbox"
              checked={canRead(status, bot.id)}
              disabled={busy || needsReconsent(status) || status.services.length === 0}
              onChange={(event) => onGrant(event.target.checked)}
              className="size-4 accent-[var(--accent)]"
            />
            Allow {bot.name} full access to {serviceList(status.services)}
          </label>
          <p className="text-[12px] text-secondary">That means it can {abilities(status.services)}. Other bots stay off until you allow them in their own panel.</p>
          <div className="flex justify-end">
            <Pill disabled={busy} className="text-danger" onClick={onDisconnect}>
              Disconnect Google
            </Pill>
          </div>
        </div>
      )}
      {canClearLocal(status) && (
        <div className="flex items-center justify-between gap-3">
          <span className="text-[12px] text-secondary">Stuck, or want to start over?</span>
          <Pill disabled={busy} onClick={onClear}>
            Clear local Google settings
          </Pill>
        </div>
      )}
      {shown && (
        <p role="alert" className="text-danger">
          {shown}
        </p>
      )}
    </div>
  );
}

// The app's own Google client (a "desktop app" OAuth client from the owner's Google Cloud project). The secret is typed into
// a password field, sent once, and gone from here: it is cleared the moment it is saved, and never stored or shown again.
function ClientForm({ busy, onSave }: { busy: boolean; onSave: (clientId: string, clientSecret: string) => void }) {
  const [id, setId] = useState("");
  const [secret, setSecret] = useState("");
  const ready = validClientId(id) && secret.length > 0;
  return (
    <form
      autoComplete="off"
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (!ready) return;
        onSave(id.trim(), secret);
        setId("");
        setSecret("");
      }}
    >
      <p>Google needs an app client first. Create a &quot;Desktop app&quot; OAuth client in your Google Cloud project and paste its details; the steps are in the Tjommie README under &quot;Connecting Google&quot;.</p>
      <div>
        <Label htmlFor="google-client-id">Client ID</Label>
        <input id="google-client-id" value={id} onChange={(event) => setId(event.target.value)} autoComplete="off" spellCheck={false} className={`${FIELD} h-10`} />
        {id.trim() !== "" && !validClientId(id) && <p className="mt-1 text-[12px] text-secondary">A client ID ends in .apps.googleusercontent.com.</p>}
      </div>
      <div>
        <Label htmlFor="google-client-secret">Client secret</Label>
        <input id="google-client-secret" type="password" value={secret} onChange={(event) => setSecret(event.target.value)} autoComplete="off" spellCheck={false} className={`${FIELD} h-10`} />
      </div>
      <div className="flex justify-end">
        <Pill primary type="submit" disabled={busy || !ready}>
          Save
        </Pill>
      </div>
    </form>
  );
}
