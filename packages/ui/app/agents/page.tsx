"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Plus, Search } from "lucide-react";
import type { Message } from "@tjommie/protocol";
import { BotCard, NewBotCard, TeamRoomCard } from "@/components/agents/cards";
import { FIELD, Segmented } from "@/components/controls";
import { IconButton } from "@/components/icon-button";
import { request } from "@/lib/connection";
import { FILTERS, matches, overviewSummary, type OverviewFilter } from "@/lib/overview";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

const NO_MESSAGES: Message[] = [];

// The Agents overview (WF2 Phase 1).
export default function AgentsPage() {
  const bots = useApp((state) => state.bots);
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
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag flex shrink-0 items-start justify-between gap-6 px-6 pb-6 pt-12">
        <div>
          <h1 className="text-[20px] font-semibold leading-[26px] text-title">Agents</h1>
          <p className="text-[13px] leading-[18px] text-secondary">{overviewSummary(bots)}</p>
        </div>
        <div className="flex items-center gap-2">
          <Segmented label="Show" value={filter} options={FILTERS} onChange={setFilter} />
          {searching ? (
            <input
              autoFocus
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onBlur={() => {
                if (!query) setSearching(false);
              }}
              placeholder="Search bots"
              aria-label="Search bots"
              className={cn(FIELD, "h-8 w-[200px] rounded-full px-3 text-[13px]")}
            />
          ) : (
            <IconButton label="Search bots" onClick={() => setSearching(true)} className="rounded-full bg-pill">
              <Search aria-hidden />
            </IconButton>
          )}
          <Link href="/bots/new/" className="press inline-flex h-8 items-center gap-1.5 rounded-full bg-user-bubble px-3.5 text-[13px] font-medium text-user-text">
            <Plus className="size-4" aria-hidden />
            New bot
          </Link>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
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
