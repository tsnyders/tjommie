"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import { Menu } from "@base-ui/react/menu";
import { CircleCheck, Ellipsis, Hourglass, Loader } from "lucide-react";
import type { AccountInfo, Bot } from "@tjommie/protocol";
import { RETRY_CAPTION, focusReady, spendingMeter, statusLine, subscriptionLabel, type StatusAction } from "@/lib/accounts";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useUsage } from "@/lib/use-usage";
import { accountUsageText } from "@/lib/usage";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { Switch } from "../controls";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { Pill } from "../pill";
import { LimitMeter, Meter } from "./limit-meter";

export type CardDialog = "rename" | "replaceKey" | "cap" | "remove";

const STUCK_MS = 8000; // a subscription still unchecked after this offers Check again instead of spinning on

// True once `waiting` has stayed true for the delay.
function useStuck(waiting: boolean): boolean {
  const [stuck, setStuck] = useState(false);
  useEffect(() => {
    if (!waiting) return;
    const timer = setTimeout(() => setStuck(true), STUCK_MS);
    return () => {
      clearTimeout(timer);
      setStuck(false);
    };
  }, [waiting]);
  return waiting && stuck;
}

const TONE: Record<string, string> = { busy: "text-body", ok: "text-body", limited: "text-body", muted: "text-secondary", problem: "text-danger" };

// One account (WF10 Accounts): its name and kind, one status line with what fixes it, its limits or spending, what it has used, and
// the choices that belong to it. `focus` is a link's wish (Sign in, Edit cap or Retry): the card scrolls into view and focuses that button,
// and does nothing else; a link never starts a sign-in or a retry.
export function AccountCard({
  account,
  siblings,
  bots,
  now,
  reopenUrl,
  focus,
  onDialog,
  onSignedIn,
}: {
  account: AccountInfo;
  siblings: AccountInfo[];
  bots: Bot[];
  now: Date;
  reopenUrl: string | null;
  focus: "signin" | "cap" | "retry" | null;
  onDialog: (dialog: CardDialog) => void;
  onSignedIn: (id: string, url: string | null) => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const stuck = useStuck(account.kind === "subscription" && account.setup === null && account.signIn === null);
  const card = useRef<HTMLElement>(null);
  const status = statusLine(account, now, { stuck, canReopen: reopenUrl !== null });
  const today = useUsage({ kind: "today", filters: { account: account.id } });
  const month = useUsage({ kind: "month", filters: { account: account.id } });
  const users = bots.filter((bot) => bot.accountId === account.id);
  const replaces = siblings.find((other) => other.id !== account.id && other.isFallback);

  // A link's wish is met once, when the control it names is there (a card that is still checking has none yet, and gets it later). It only
  // focuses; no call is made.
  const ready = focus !== null && focusReady(focus, account, status.actions);
  const focused = useRef<string | null>(null);
  useEffect(() => {
    if (!focus) {
      focused.current = null;
      return;
    }
    if (!ready || focused.current === focus) return;
    focused.current = focus;
    card.current?.scrollIntoView({ block: "center" });
    card.current?.querySelector<HTMLElement>(`[data-account-action="${focus}"]`)?.focus();
  }, [focus, ready]);

  async function run(action: () => Promise<unknown>): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  const signIn = (): Promise<void> =>
    run(async () => {
      const started = await request("accounts.signIn", { id: account.id });
      onSignedIn(account.id, started.url);
    });
  const update = (patch: Parameters<typeof request<"accounts.update">>[1]["patch"]): Promise<void> => run(() => request("accounts.update", { id: account.id, patch }));
  const act = (action: StatusAction): ReactNode => {
    switch (action) {
      case "openAgain":
        return (
          <a key={action} href={reopenUrl ?? undefined} target="_blank" rel="noopener noreferrer" className="text-[13px] text-accent hover:underline">
            Open the page again
          </a>
        );
      case "cancel":
        return (
          <Pill key={action} disabled={busy} onClick={() => account.signIn && void run(() => request("accounts.cancelSignIn", { id: account.id, attempt: account.signIn?.attempt ?? "" }))}>
            Cancel
          </Pill>
        );
      case "tryAgain":
        return (
          <Pill key={action} primary disabled={busy} data-account-action="signin" onClick={() => void signIn()}>
            Try again
          </Pill>
        );
      case "check":
        return (
          <Pill key={action} disabled={busy} onClick={() => void run(() => request("accounts.check", { id: account.id }))}>
            Check again
          </Pill>
        );
      case "retryLimit":
        return (
          <Pill key={action} primary disabled={busy} data-account-action="retry" onClick={() => void run(() => request("accounts.retryLimit", { id: account.id }))}>
            Retry queued work
          </Pill>
        );
      case "signIn":
        return (
          <Pill key={action} primary disabled={busy} data-account-action="signin" onClick={() => void signIn()}>
            Sign in
          </Pill>
        );
      case "addKey":
        return (
          <Pill key={action} primary onClick={() => onDialog("replaceKey")}>
            Add key
          </Pill>
        );
      case "raiseCap":
        return (
          <Pill key={action} primary data-account-action="cap" onClick={() => onDialog("cap")}>
            Raise cap
          </Pill>
        );
    }
  };

  const icon =
    status.tone === "busy" ? <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-hidden /> : status.tone === "ok" ? <CircleCheck className="size-3.5 shrink-0 text-working" aria-hidden /> : status.tone === "limited" ? <Hourglass className="size-3.5 shrink-0 text-icon" aria-hidden /> : null;
  const spending = account.kind === "api_key" ? spendingMeter(account.spending) : null;

  return (
    <section ref={card} aria-label={account.label} className="flex max-w-[760px] flex-col gap-3 rounded-[12px] bg-card p-4">
      <div className="flex items-center gap-2">
        <h4 className="min-w-0 truncate text-[15px] font-semibold leading-[20px] text-title">{account.label}</h4>
        {account.isDefault && <span className="flex h-[22px] shrink-0 items-center rounded-full bg-pill px-2 text-[12px] text-text">Default</span>}
        <span className="flex-1" />
        <AccountMenu account={account} busy={busy} onDialog={onDialog} onSignIn={() => void signIn()} onDefault={() => void update({ isDefault: true })} />
      </div>
      <p className="-mt-2 text-[12px] leading-[18px] text-secondary">{account.kind === "api_key" ? "API key" : subscriptionLabel(account)}</p>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <p className={cn("flex min-w-0 flex-1 items-center gap-2 text-[13px] leading-[18px] max-sm:min-w-[180px]", TONE[status.tone])} role={status.tone === "problem" ? "alert" : undefined}>
          {icon}
          <span className="min-w-0 break-words">{status.text}</span>
        </p>
        {status.actions.length > 0 && <div className="flex flex-wrap items-center gap-2 sm:shrink-0">{status.actions.map(act)}</div>}
      </div>
      {status.actions.includes("retryLimit") && <p className="-mt-1 text-[12px] leading-[18px] text-secondary">{RETRY_CAPTION}</p>}
      {problem && (
        <p role="alert" className="text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      {account.kind === "subscription" &&
        (account.limits.length > 0 ? (
          <div className="flex flex-col gap-2.5">
            {account.limits.map((window) => (
              <LimitMeter key={`${window.limitId}:${window.windowKey}`} window={window} now={now} />
            ))}
          </div>
        ) : account.setup?.signedIn ? (
          <p className="text-[12px] leading-[18px] text-secondary">Limits show after this account&apos;s first message.</p>
        ) : null)}
      {spending && (
        <div className="flex items-end gap-3">
          <div className="min-w-0 flex-1">
            <Meter label="Spending" value={spending.text} share={spending.share} near={spending.near} />
          </div>
          <button type="button" data-account-action="cap" onClick={() => onDialog("cap")} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            Edit cap
          </button>
        </div>
      )}
      <p className="text-[12px] leading-[18px] text-secondary">
        Today {today.report ? accountUsageText(today.report.summary, account.kind) : "…"} · This month {month.report ? accountUsageText(month.report.summary, account.kind) : "…"}
      </p>
      {account.kind === "api_key" && (
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <p className="text-[13px] leading-[18px] text-text">Fallback for {account.provider === "claude" ? "Claude" : "Codex"}</p>
            <p className="text-[12px] leading-[18px] text-secondary">
              When a {account.provider === "claude" ? "Claude" : "Codex"} subscription account hits its limit, bots carry on with this key until it resets.
              {!account.isFallback && replaces ? ` Replaces ${replaces.label}.` : ""}
            </p>
          </div>
          <Switch label={`Fallback for ${account.provider === "claude" ? "Claude" : "Codex"}`} checked={account.isFallback} disabled={busy} onChange={(enabled) => void update({ isFallback: enabled })} />
        </div>
      )}
      {users.length > 0 && (
        <div className="flex items-center gap-1.5 text-[12px] leading-[18px] text-secondary">
          <span className="flex -space-x-1">
            {users.slice(0, 5).map((bot) => (
              <BotAvatar key={bot.id} color={bot.color} shape={bot.shape} size={18} />
            ))}
          </span>
          {users.length} {users.length === 1 ? "bot" : "bots"}
        </div>
      )}
    </section>
  );
}

function AccountMenu({ account, busy, onDialog, onSignIn, onDefault }: { account: AccountInfo; busy: boolean; onDialog: (dialog: CardDialog) => void; onSignIn: () => void; onDefault: () => void }) {
  const label = `More for ${account.label}`;
  return (
    <Menu.Root>
      <Menu.Trigger aria-label={label} title={label} className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text data-[popup-open]:bg-selected">
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            <Menu.Item className={MENU_ITEM} onClick={() => onDialog("rename")}>
              Rename
            </Menu.Item>
            {!account.isDefault && (
              <Menu.Item className={MENU_ITEM} disabled={busy} onClick={onDefault}>
                Make default
              </Menu.Item>
            )}
            {account.kind === "subscription" && account.setup?.signedIn && (
              <Menu.Item className={MENU_ITEM} disabled={busy} onClick={onSignIn}>
                Use a different account
              </Menu.Item>
            )}
            {account.kind === "api_key" && (
              <>
                <Menu.Item className={MENU_ITEM} onClick={() => onDialog("replaceKey")}>
                  Replace key
                </Menu.Item>
                <Menu.Item className={MENU_ITEM} onClick={() => onDialog("cap")}>
                  Edit cap
                </Menu.Item>
              </>
            )}
            <Menu.Item className={cn(MENU_ITEM, "text-danger")} onClick={() => onDialog("remove")}>
              Remove
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
