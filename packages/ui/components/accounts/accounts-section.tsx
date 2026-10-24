"use client";

import { useEffect, useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { Plus } from "lucide-react";
import type { AccountInfo, Provider } from "@tjommie/protocol";
import { accountsOf, closeEpoch, rememberUrl } from "@/lib/accounts";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { PROVIDER_NAME } from "@/lib/setup";
import { useApp } from "@/lib/store";
import { AccountCard, type CardDialog } from "./account-card";
import { AddKeyDialog, AddSubscriptionDialog, CapDialog, RemoveDialog, RenameDialog, ReplaceKeyDialog } from "./account-dialogs";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { Pill } from "../pill";

const PROVIDERS: Provider[] = ["claude", "codex"];
const MINUTE_MS = 60_000;

type Dialog = { kind: "subscription" | "key"; provider: Provider } | { kind: CardDialog; id: string };
type Open = (Dialog & { epoch: number }) | null;

// Settings: Accounts. Bots run Claude Code and Codex on these accounts; subscriptions sign in on the provider's own page and API keys stay
// in Windows Credential Manager. `account` and `action` come from a link (a banner's Sign in or Raise cap): they point at a card and never
// start anything.
export function AccountsSection({ account, action }: { account: string | null; action: "signin" | "cap" | "retry" | null }) {
  const accounts = useApp((state) => state.accounts);
  const bots = useApp((state) => state.bots);
  const setup = useApp((state) => state.setup);
  const [open, setOpen] = useState<Open>(null);
  const epochs = useRef(0);
  const show = (dialog: Dialog): void => setOpen({ ...dialog, epoch: (epochs.current += 1) });
  const [urls, setUrls] = useState<Record<string, string>>({}); // sign-in pages by account, kept so "Open the page again" has one
  const [problem, setProblem] = useState<string | null>(null);
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), MINUTE_MS);
    return () => clearInterval(timer);
  }, []);
  // A subscription whose first check hasn't come back is checked once when this section opens (the card offers Check again if it
  // still hasn't answered, so nothing waits forever).
  const checked = useRef(false);
  useEffect(() => {
    if (checked.current || !accounts?.some((each) => each.kind === "subscription" && each.setup === null)) return;
    checked.current = true;
    request("accounts.check", {}).catch(() => {});
  }, [accounts]);

  // The daemon opens the sign-in page itself. The address is only kept, so "Open the page again" is something the person chooses.
  const signedIn = (id: string, url: string | null): void => setUrls((current) => rememberUrl(current, id, url));
  const byId = (id: string): AccountInfo | undefined => accounts?.find((each) => each.id === id);
  const target = open && "id" in open ? byId(open.id) : undefined;
  // Each dialog closes only itself: a save that finishes after another dialog was opened must not close that one.
  const closer = (epoch: number) => (): void => setOpen((current) => closeEpoch(current, epoch));

  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">Accounts</h2>
      <p className="mt-2 max-w-[700px] text-[13px] leading-[18px] text-secondary">
        Bots run Claude Code and Codex on these accounts. Subscriptions sign in on the provider&apos;s own page; Tjommie never sees your password. API keys stay in Windows Credential Manager.
      </p>
      {problem && (
        <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      {!accounts ? (
        <p className="mt-8 text-[13px] leading-[18px] text-secondary">Loading accounts</p>
      ) : (
        PROVIDERS.map((provider) => (
          <Group
            key={provider}
            provider={provider}
            accounts={accountsOf(accounts, provider)}
            fallbackSetup={setup?.find((entry) => entry.provider === provider)}
            render={(each, siblings) => (
              <AccountCard
                key={each.id}
                account={each}
                siblings={siblings}
                bots={bots}
                now={now}
                reopenUrl={urls[each.id] ?? null}
                focus={account === each.id ? action : null}
                onDialog={(kind) => show({ kind, id: each.id })}
                onSignedIn={signedIn}
              />
            )}
            onAdd={(kind) => show({ kind, provider })}
          />
        ))
      )}
      {open?.kind === "subscription" && (
        <AddSubscriptionDialog
          provider={open.provider}
          onClose={closer(open.epoch)}
          onSubmit={async (label) => {
            const created = await request("accounts.create", { provider: open.provider, label, kind: "subscription" });
            // The account exists now. If its sign-in can't start, the card says so and offers Try again; creating it again would only collide.
            try {
              const started = await request("accounts.signIn", { id: created.id });
              signedIn(created.id, started.url);
            } catch (error) {
              setProblem(errorText(error));
            }
          }}
        />
      )}
      {open?.kind === "key" && <AddKeyDialog provider={open.provider} onClose={closer(open.epoch)} onSubmit={async (key) => void (await request("accounts.create", { provider: open.provider, kind: "api_key", ...key }))} />}
      {target && open?.kind === "rename" && <RenameDialog account={target} onClose={closer(open.epoch)} onSubmit={async (label) => void (await request("accounts.update", { id: target.id, patch: { label } }))} />}
      {target && open?.kind === "replaceKey" && <ReplaceKeyDialog account={target} onClose={closer(open.epoch)} onSubmit={async (apiKey) => void (await request("accounts.update", { id: target.id, patch: { apiKey } }))} />}
      {target && open?.kind === "cap" && <CapDialog account={target} onClose={closer(open.epoch)} onSubmit={async (spendingCapUsd) => void (await request("accounts.update", { id: target.id, patch: { spendingCapUsd } }))} />}
      {target && open?.kind === "remove" && <RemoveDialog account={target} bots={bots} onClose={closer(open.epoch)} onSubmit={async () => void (await request("accounts.remove", { id: target.id }))} />}
    </>
  );
}

function Group({
  provider,
  accounts,
  fallbackSetup,
  render,
  onAdd,
}: {
  provider: Provider;
  accounts: AccountInfo[];
  fallbackSetup: { installed: boolean; version: string | null; sandbox?: string } | undefined;
  render: (account: AccountInfo, siblings: AccountInfo[]) => React.ReactNode;
  onAdd: (kind: "subscription" | "key") => void;
}) {
  const setup = accounts.find((each) => each.setup)?.setup ?? fallbackSetup;
  const installed = setup ? setup.installed : true;
  const sandbox = setup && "sandbox" in setup ? setup.sandbox : undefined;
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const first = accounts[0];
  const name = provider === "claude" ? "Claude" : "Codex";
  return (
    <section aria-label={PROVIDER_NAME[provider]} className="mt-9">
      <div className="flex items-center gap-3">
        <h3 className="text-[15px] font-semibold leading-[20px] text-title">{PROVIDER_NAME[provider]}</h3>
        {setup?.version && <span className="text-[12px] leading-[18px] text-secondary">Version {setup.version}</span>}
        <span className="flex-1" />
        <Menu.Root>
          <Menu.Trigger disabled={!installed} aria-label={`Add ${name} account`} className="press inline-flex h-8 items-center gap-1.5 rounded-full bg-pill px-3.5 text-[14px] font-medium text-text disabled:opacity-50">
            <Plus className="size-4" aria-hidden />
            Add
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner side="bottom" align="end" sideOffset={6}>
              <Menu.Popup className={MENU_POPUP}>
                <Menu.Item className={MENU_ITEM} onClick={() => onAdd("subscription")}>
                  Subscription
                </Menu.Item>
                <Menu.Item className={MENU_ITEM} onClick={() => onAdd("key")}>
                  API key
                </Menu.Item>
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
      </div>
      {!installed && (
        <p className="mt-3 max-w-[700px] text-[13px] leading-[18px] text-secondary">
          {provider === "claude" ? (
            <>
              Install Claude Code from{" "}
              <a href="https://claude.com/claude-code" target="_blank" rel="noreferrer" className="text-accent hover:underline">
                claude.com/claude-code
              </a>
              , then check again.
            </>
          ) : (
            <>
              Install Codex with <code className="rounded-[6px] bg-code px-1 py-0.5 text-[12px]">npm install -g @openai/codex</code>, then check again.
            </>
          )}
        </p>
      )}
      {installed && provider === "codex" && sandbox !== undefined && sandbox !== "ready" && (
        <div className="mt-3 flex max-w-[700px] flex-wrap items-center gap-3 text-[13px] leading-[18px] text-secondary">
          <p className="min-w-0 flex-1">Codex&apos;s sandbox isn&apos;t set up yet. Codex bots at Own folder need it, and Windows asks for admin permission once.</p>
          {first && (
            <Pill
              primary
              disabled={busy}
              onClick={() => {
                setBusy(true);
                setProblem(null);
                request("accounts.prepareCodexSandbox", { id: first.id })
                  .catch((error: unknown) => setProblem(errorText(error)))
                  .finally(() => setBusy(false));
              }}
            >
              Set up Codex&apos;s sandbox
            </Pill>
          )}
        </div>
      )}
      {problem && (
        <p role="alert" className="mt-2 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      {accounts.length === 0 ? (
        <p className="mt-3 text-[13px] leading-[18px] text-secondary">No {name} accounts yet.</p>
      ) : (
        <div className="mt-3 flex flex-col gap-4">{accounts.map((each) => render(each, accounts))}</div>
      )}
    </section>
  );
}
