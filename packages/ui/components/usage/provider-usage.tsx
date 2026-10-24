"use client";

import { RefreshCw } from "lucide-react";
import type { AccountInfo } from "@tjommie/protocol";
import { providerName, providerView, quotaCard } from "@/lib/accounts";
import { cn } from "@/lib/utils";
import { Meter } from "../accounts/limit-meter";
import { IconButton } from "../icon-button";

// A subscription account's quota card at the top of Usage: the provider, the plan if known, each limit window with its use and reset, and
// who is signed in. A refresh asks the provider's own free quota check; it never signs in and never runs a paid turn. Extra usage and Credits show
// only when the provider reported them, with or without limit windows, and are never guessed.
export function ProviderUsage({ accounts, now, busy, problem, onRefresh }: { accounts: AccountInfo[]; now: Date; busy: string | null; problem: string | null; onRefresh: (id?: string) => void }) {
  const subscriptions = accounts.filter((account) => account.kind === "subscription");
  if (subscriptions.length === 0) return null;
  return (
    <section aria-label="Provider usage" className="mt-5 flex max-w-[760px] flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-[13px] font-semibold leading-[18px] text-label">Subscriptions</h3>
        <IconButton label="Refresh all limits" disabled={busy !== null} onClick={() => onRefresh()} className={cn(busy === "all" && "[&_svg]:animate-spin")}>
          <RefreshCw aria-hidden />
        </IconButton>
      </div>
      {problem && (
        <p role="alert" className="text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
      {subscriptions.map((account) => (
        <ProviderCard key={account.id} account={account} now={now} busy={busy === account.id} disabled={busy !== null} onRefresh={() => onRefresh(account.id)} />
      ))}
    </section>
  );
}

function ProviderCard({ account, now, busy, disabled, onRefresh }: { account: AccountInfo; now: Date; busy: boolean; disabled: boolean; onRefresh: () => void }) {
  const card = quotaCard(account);
  const view = providerView(account, now);
  const plan = view.plan;
  return (
    <section aria-label={`${providerName(account.provider)}, ${account.label}`} className="flex flex-col gap-3 rounded-[12px] bg-card p-4">
      <div className="flex items-center gap-2">
        <h4 className="min-w-0 truncate text-[15px] font-semibold leading-[20px] text-title">{providerName(account.provider)}</h4>
        {/* The plan can be long (up to 120 characters), so it is cut short here and the refresh stays inside the card. */}
        {plan && (
          <span title={plan} className="flex h-[22px] min-w-0 max-w-[45%] items-center rounded-full bg-pill px-2 text-[12px] text-text">
            <span className="truncate">{plan}</span>
          </span>
        )}
        <span className="flex-1" />
        <IconButton label={`Refresh ${account.label}`} disabled={disabled} onClick={onRefresh} className={cn(busy && "[&_svg]:animate-spin")}>
          <RefreshCw aria-hidden />
        </IconButton>
      </div>
      {card.kind === "limits" && (
        <>
          {view.rows.length > 0 ? (
            <div className="flex flex-col gap-2.5">
              {view.rows.map((row) => (
                <Meter key={row.key} label={row.label} value={row.value} share={row.share} near={row.near} healthy="bg-working" />
              ))}
            </div>
          ) : (
            <p className="text-[12px] leading-[18px] text-secondary">Limits aren&apos;t available for this account yet.</p>
          )}
          {view.extras.map((extra) => (
            <p key={extra.label} className="flex items-baseline justify-between gap-3 text-[12px] leading-[18px]">
              <span className="text-secondary">{extra.label}</span>
              <span className="text-body">{extra.value}</span>
            </p>
          ))}
          {view.stale && <p className="text-[12px] leading-[18px] text-secondary">{view.stale}</p>}
          {view.problem && (
            <p role="alert" className="text-[12px] leading-[18px] text-danger">
              {view.problem}
            </p>
          )}
        </>
      )}
      {card.kind === "unavailable" && <p className="text-[12px] leading-[18px] text-secondary">Limits aren&apos;t available for this account yet. They show after its first message, or when you refresh.</p>}
      {card.kind === "checking" && <p className="text-[12px] leading-[18px] text-secondary">Checking</p>}
      {card.kind === "signedOut" && <p className="text-[12px] leading-[18px] text-secondary">Not signed in, so there are no limits to show.</p>}
      {card.kind === "problem" && (
        <p role="alert" className="text-[12px] leading-[18px] text-danger">
          {card.text}
        </p>
      )}
      <p className="truncate text-[12px] leading-[18px] text-secondary">{[account.label, account.setup?.account].filter(Boolean).join(" · ")}</p>
    </section>
  );
}
