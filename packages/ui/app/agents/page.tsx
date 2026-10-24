"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Plus, Search } from "lucide-react";
import type { Message } from "@tjommie/protocol";
import { BotCard, NeedsYouCard, NewBotCard, TeamRoomCard } from "@/components/agents/cards";
import { FIELD, Segmented } from "@/components/controls";
import { IconButton } from "@/components/icon-button";
import { request } from "@/lib/connection";
import { FILTERS, matches, overviewLede, overviewSummary, phoneFilters, type OverviewFilter } from "@/lib/overview";
import { useApp } from "@/lib/store";
import { usePhone } from "@/lib/use-phone";
import { phoneCaptionClear } from "@/lib/native-capability";
import { cn } from "@/lib/utils";

const NO_MESSAGES: Message[] = [];

// The Agents overview (WF2 Phase 1).
export default function AgentsPage() {
  const bots = useApp((state) => state.bots);
  const phone = usePhone();
  const routines = useApp((state) => state.routines);
  const ready = useApp((state) => state.connection.status === "ready");
  const [filter, setFilter] = useState<OverviewFilter>("all");
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [digests, setDigests] = useState<Record<string, Message[]>>({});
  const [room, setRoom] = useState<Message[]>(NO_MESSAGES);

  // Each card reads its bot's newest messages: fetched when the page opens, and again whenever a bot changes.
  useEffect(() => {
    if (!ready) return;
    let live = true;
    for (const bot of bots) {
      request("chat.history", { botId: bot.id, limit: 30 }).then(
        ({ messages }) => {
          if (live) setDigests((current) => ({ ...current, [bot.id]: messages }));
        },
        () => {},
      );
    }
    request("room.history", { limit: 10 }).then(
      ({ messages }) => {
        if (live) setRoom(messages);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [bots, ready]);

  const shown = bots.filter((bot) => matches(bot, filter, query));
  const everything = filter === "all" && !query.trim();
  const chips = phone ? phoneFilters(bots, filter) : [];
  const needsYou = bots.find((bot) => bot.state === "waiting");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag shrink-0 px-6 pb-7 pt-12 max-sm:px-4 max-sm:pb-3 max-sm:pt-4">
        {/* The top row (title, search, +) is what the caption buttons sit over, so it alone clears them; the filter below has the full width. */}
        <div className={cn("flex items-center justify-between gap-6 max-sm:gap-2", phoneCaptionClear())}>
        <div className="max-sm:min-w-0 max-sm:flex-1">
          {/* Folio's date line and lede; the other styles and the desktop never show them. */}
          {phone && (
            <p data-slot="date" className="style-folio truncate">
              {new Date().toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" })}
            </p>
          )}
          <h1 data-slot="title" className="truncate text-[20px] font-semibold leading-[26px] text-title">Agents</h1>
          <p data-slot="summary" className="mt-1 text-[13px] leading-[18px] text-secondary">{overviewSummary(bots)}</p>
        </div>
        <div className="flex items-center gap-2">
          {!phone && <Segmented label="Show" value={filter} options={FILTERS} onChange={setFilter} />}
          {searching && !phone ? (
            <SearchField query={query} onQuery={setQuery} onDone={() => setSearching(false)} className="h-8 w-[200px]" />
          ) : searching ? null : (
            <IconButton label="Search bots" onClick={() => setSearching(true)} className="rounded-full bg-pill max-sm:size-11">
              <Search aria-hidden />
            </IconButton>
          )}
          <Link href="/bots/new/" aria-label="New bot" data-slot="new-bot" className="press inline-flex h-8 items-center gap-1.5 rounded-full bg-user-bubble px-3 text-[14px] font-medium text-user-text max-sm:size-11 max-sm:justify-center max-sm:px-0">
            <Plus className="size-3.5 max-sm:size-5" aria-hidden />
            <span className="max-sm:sr-only">New bot</span>
          </Link>
        </div>
        </div>
        {/* Folio's lede has the full width under the caption-cleared row, at most two lines, so the header stays short whatever the names. */}
        {phone && (
          <p data-slot="lede" className="style-folio line-clamp-2 [overflow-wrap:anywhere]">
            {overviewLede(bots)}
          </p>
        )}
        {/* On a phone the open search box has a row of its own under the title, so it never crowds the caption buttons or New bot. */}
        {phone && searching && <SearchField query={query} onQuery={setQuery} onDone={() => setSearching(false)} className="mt-3 h-11 w-full" />}
        {/* On a phone the filter is a row of quiet chips with counts, only for states some bot is in, and gone when none would narrow the list.
            Folio draws the same buttons as underlined text tabs. */}
        {chips.length > 0 && (
          <div role="group" aria-label="Show" data-slot="filters" className="-mx-4 mt-3 flex gap-2 overflow-x-auto px-4 [scrollbar-width:none]">
            {chips.map((chip) => (
              <button
                key={chip.value}
                type="button"
                aria-pressed={filter === chip.value}
                data-slot="filter"
                onClick={() => setFilter(chip.value)}
                className="mobile-motion-choice flex h-11 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border border-divider px-3.5 text-[13px] text-secondary aria-pressed:border-transparent aria-pressed:bg-selected aria-pressed:font-medium aria-pressed:text-text"
              >
                {chip.label}
                <span data-slot="filter-count" className="tabular-nums text-caption">
                  {chip.count}
                </span>
              </button>
            ))}
          </div>
        )}
      </header>
      <div data-nav-clear className="min-h-0 flex-1 overflow-y-auto px-6 pb-6 max-sm:px-4">
        {phone && everything && needsYou && <NeedsYouCard bot={needsYou} messages={digests[needsYou.id] ?? NO_MESSAGES} />}
        <div data-slot="bot-list" className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4 max-sm:gap-2">
          {shown.map((bot) => (
            <BotCard key={bot.id} bot={bot} bots={bots} messages={digests[bot.id] ?? NO_MESSAGES} routines={routines} />
          ))}
          {everything && <TeamRoomCard messages={room} bots={bots} />}
          {everything && <NewBotCard />}
        </div>
        {!everything && shown.length === 0 && <p className="mt-10 text-center text-[13px] text-secondary">No bots match.</p>}
      </div>
    </div>
  );
}

// The search box: it opens focused, and closes when left empty.
function SearchField({ query, onQuery, onDone, className }: { query: string; onQuery: (query: string) => void; onDone: () => void; className: string }) {
  return (
    <input
      autoFocus
      value={query}
      onChange={(event) => onQuery(event.target.value)}
      onBlur={() => {
        if (!query) onDone();
      }}
      placeholder="Search bots"
      aria-label="Search bots"
      className={cn(FIELD, "rounded-full px-3 text-[13px]", className)}
    />
  );
}
