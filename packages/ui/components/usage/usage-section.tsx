"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { X } from "lucide-react";
import type { AccountInfo, Bot, UsageAggregate, UsageReport } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { usePhone } from "@/lib/use-phone";
import { useUsage } from "@/lib/use-usage";
import { cn } from "@/lib/utils";
import { breakdownRows, chartData, combinedCost, costLines, costText, dayLabel, emptyUsage, labelled, monthLabel, tokensText, usageHref, type By, type RangeKind, type UsageFilters } from "@/lib/usage";
import { ProviderUsage } from "./provider-usage";
import { Segmented } from "../controls";

const RANGES = [
  { value: "today", label: "Today" },
  { value: "week", label: "This week" },
  { value: "month", label: "This month" },
  { value: "year", label: "This year" },
] as const satisfies readonly { value: RangeKind; label: string }[];

const BREAKDOWNS = [
  { value: "bot", label: "By bot" },
  { value: "account", label: "By account" },
  { value: "model", label: "By model" },
] as const satisfies readonly { value: By; label: string }[];

function Tile({ label, value, note }: { label: string; value: string; note?: string | null }) {
  return (
    <div className="min-w-[150px] flex-1 rounded-[12px] bg-card p-4">
      <p className="text-[12px] leading-[18px] text-secondary">{label}</p>
      <p className="mt-0.5 text-[20px] font-semibold leading-[26px] text-title">{value}</p>
      {note && <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">{note}</p>}
    </div>
  );
}

// Settings: Usage. Tokens, what the API keys spent, and what the subscriptions' tokens would have cost on the API, each on its own: they
// are different things and are never added together. A cost that isn't fully known says so instead of showing $0.
// The provider cards with their refresh: one account's quota check, or all of them. It asks the provider's own free check and nothing else.
function ProviderUsageSection({ accounts }: { accounts: AccountInfo[] }) {
  const [busy, setBusy] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [now, setNow] = useState(() => new Date());
  // Resets are shown against the time now, so it keeps moving: every minute, and when a refresh has come back (a window may have reset overnight).
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 60_000);
    return () => clearInterval(timer);
  }, []);
  const refresh = (id?: string): void => {
    setBusy(id ?? "all");
    setProblem(null);
    request("accounts.check", id ? { id } : {})
      .catch((error: unknown) => setProblem(errorText(error)))
      .finally(() => {
        setBusy(null);
        setNow(new Date());
      });
  };
  return <ProviderUsage accounts={accounts} now={now} busy={busy} problem={problem} onRefresh={refresh} />;
}

// On a phone the range is a row of 44 px chips, as the Agents filter is, rather than the thin segmented track; the phone styles restyle the same
// data-slot hooks (Folio's text tabs, Aurora's glass). Narrow phones get tighter chips so all four fit without scrolling.
export function RangeChips({ value, onChange }: { value: RangeKind; onChange: (range: RangeKind) => void }) {
  return (
    <div role="group" aria-label="Range" data-slot="filters" className="flex w-full gap-2 overflow-x-auto [scrollbar-width:none] max-[380px]:gap-1.5">
      {RANGES.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          data-slot="filter"
          onClick={() => onChange(option.value)}
          className="mobile-motion-choice flex h-11 shrink-0 items-center whitespace-nowrap rounded-full border border-divider px-3.5 text-[13px] text-secondary aria-pressed:border-transparent aria-pressed:bg-selected aria-pressed:font-medium aria-pressed:text-text max-[380px]:px-2.5"
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function UsageSection({ filters }: { filters: UsageFilters }) {
  const phone = usePhone();
  const [range, setRange] = useState<RangeKind>("week");
  const [by, setBy] = useState<By>("bot");
  const bots = useApp((state) => state.bots);
  const accounts = useApp((state) => state.accounts);
  const { report, error } = useUsage({ kind: range, filters });
  const chips: { key: keyof UsageFilters; label: string }[] = [];
  if (filters.bot) chips.push({ key: "bot", label: bots.find((bot) => bot.id === filters.bot)?.name ?? "Deleted bot" });
  if (filters.account) chips.push({ key: "account", label: accounts?.find((account) => account.id === filters.account)?.label ?? "Removed account" });
  if (filters.model) chips.push({ key: "model", label: filters.model });

  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">Usage</h2>
      <div className="mt-5 flex flex-wrap items-center gap-3">
        {phone ? <RangeChips value={range} onChange={setRange} /> : <Segmented label="Range" value={range} options={RANGES} onChange={setRange} />}
        {chips.map((chip) => (
          <span key={chip.key} className="flex h-8 items-center gap-1.5 rounded-full bg-pill pl-3 pr-1.5 text-[13px] text-text">
            {chip.label}
            <Link href={usageHref({ ...filters, [chip.key]: undefined })} aria-label={`Remove the ${chip.key} filter`} className="flex size-5 items-center justify-center rounded-full text-icon hover:bg-selected hover:text-text">
              <X className="size-3.5" aria-hidden />
            </Link>
          </span>
        ))}
      </div>
      {accounts && <ProviderUsageSection accounts={accounts} />}
      {error && (
        <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
          {error}
        </p>
      )}
      {!report ? (
        !error && <p className="mt-8 text-[13px] leading-[18px] text-secondary">Loading usage</p>
      ) : (
        <UsageReportView report={report} range={range} by={by} onBy={setBy} bots={bots} accounts={accounts ?? []} />
      )}
    </>
  );
}

// The known costs added into one figure, labelled for what it is: the API-equivalent value of what was used, not what a subscription costs.
function CombinedTile({ costs }: { costs: UsageAggregate["costs"] }) {
  const text = costText(combinedCost(costs));
  return <Tile label="Combined cost estimate" value={text.text} note={[text.note, "API-equivalent value, not your subscription bill"].filter(Boolean).join(" · ")} />;
}

export function UsageReportView({ report, range, by, onBy, bots, accounts }: { report: UsageReport; range: RangeKind; by: By; onBy: (by: By) => void; bots: Pick<Bot, "id" | "name">[]; accounts: Pick<AccountInfo, "id" | "label">[] }) {
  if (emptyUsage(report)) return <p className="mt-8 text-[13px] leading-[18px] text-secondary">No usage yet. It shows here after your bots&apos; first messages.</p>;
  return (
    <>
      <div className="mt-5 flex max-w-[760px] flex-wrap gap-3">
        <Tile label="Tokens" value={tokensText(report.summary.tokens.total)} />
        <CombinedTile costs={report.summary.costs} />
        <CostTile label="Spent (API keys)" cost={report.summary.costs.apiSpend} />
        <CostTile label="API value (subscriptions)" cost={report.summary.costs.apiValue} />
        {report.summary.costs.unclassified.knownRecords + report.summary.costs.unclassified.unknownRecords > 0 && <CostTile label="Other" cost={report.summary.costs.unclassified} />}
      </div>
      {range !== "today" && <Bars report={report} range={range} />}
      <div className="mt-8 flex items-center justify-between gap-3">
        <h3 className="text-[13px] font-semibold leading-[18px] text-label">Breakdown</h3>
        <Segmented label="Breakdown" value={by} options={BREAKDOWNS} onChange={onBy} />
      </div>
      <ul className="mt-3 flex max-w-[760px] flex-col gap-2">
        {breakdownRows(report, by, bots, accounts).map((row) => (
          <li key={row.key} className="rounded-[12px] bg-card px-4 py-3">
            <div className="flex items-baseline justify-between gap-3">
              <span className="min-w-0 truncate text-[14px] leading-[21px] text-text">{row.name}</span>
              <span className="shrink-0 text-[13px] text-body">{tokensText(row.aggregate.tokens.total)} tokens</span>
            </div>
            <CostLines aggregate={row.aggregate} />
          </li>
        ))}
      </ul>
    </>
  );
}

function CostTile({ label, cost }: { label: string; cost: UsageAggregate["costs"]["apiSpend"] }) {
  const text = costText(cost);
  return <Tile label={label} value={text.text} note={text.note} />;
}

function CostLines({ aggregate }: { aggregate: UsageAggregate }) {
  const lines = costLines(aggregate);
  if (lines.length === 0) return null;
  return (
    <div className="mt-0.5 flex flex-col text-[12px] leading-[18px] text-secondary">
      {lines.map((line) => (
        <span key={line.label}>
          {labelled(line.label, line.text)}
          {line.note ? ` · ${line.note}` : ""}
        </span>
      ))}
    </div>
  );
}

// Tokens per day as plain bars, a slot for every calendar day of the range (the quiet ones empty), the busiest day full height, with a few dates
// under them and the zone the days are in. No chart library: it is a picture with a sentence for anyone who can't see it.
function Bars({ report, range }: { report: UsageReport; range: RangeKind }) {
  const { bars, label, ticks, unit } = chartData(report, range);
  if (bars.length === 0) return null;
  return (
    <div className="mt-5 max-w-[760px] rounded-[12px] bg-card p-4">
      <div role="img" aria-label={label} className="flex h-24 items-end">
        {bars.map((bar) => (
          <div key={bar.day} title={unit === "month" ? monthLabel(bar.day) : dayLabel(bar.day)} className={cn("flex h-full min-w-0 flex-1 items-end", unit === "month" ? "px-0.5" : "px-px")}>
            <div className="w-full rounded-t-[3px] bg-selected" style={{ height: `${bar.share > 0 ? Math.max(bar.share * 100, 4) : 1}%` }} />
          </div>
        ))}
      </div>
      <div aria-hidden className="relative mt-1.5 h-4 text-[11px] leading-[16px] text-secondary">
        {ticks.map((tick, index, all) => (
          <span key={tick.index} className="absolute whitespace-nowrap" style={{ left: `${((tick.index + 0.5) / bars.length) * 100}%`, transform: index === 0 ? "none" : index === all.length - 1 ? "translateX(-100%)" : "translateX(-50%)" }}>
            {tick.label}
          </span>
        ))}
      </div>
      <p className="mt-1 text-[11px] leading-[16px] text-secondary">{unit === "month" ? "Months" : "Days"} in {report.timeZone}</p>
    </div>
  );
}
