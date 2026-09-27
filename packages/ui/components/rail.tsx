"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { useEffect, useState, type ReactNode } from "react";
import { Hourglass, LayoutGrid, Plug, Plus, Settings } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { desktop } from "@/lib/desktop";
import { stateLabel } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar, TeamMark } from "./bot-avatar";

export function Rail() {
  const bots = useApp((state) => state.bots);
  const pathname = usePathname();
  const params = useSearchParams();
  const openBot = pathname.startsWith("/chat") ? params.get("bot") : null;
  const [initial, setInitial] = useState("");
  useEffect(() => {
    void desktop()
      ?.info()
      .then((info) => setInitial(info.username.slice(0, 1).toUpperCase()));
  }, []);
  return (
    <nav aria-label="Tjommie" className="flex w-[88px] shrink-0 flex-col items-center border-r border-rail-edge bg-rail pt-2.5 pb-4">
      <RailLink href="/room/" label="Team room" selected={pathname.startsWith("/room")}>
        <TeamMark size={40} dimmed={bots.length === 0} />
      </RailLink>
      <div className="mt-1.5 mb-[5px] h-px w-8 shrink-0 bg-divider" />
      <div className="flex min-h-0 w-full flex-1 flex-col items-center overflow-y-auto">
        {bots.map((bot) => (
          <BotLink key={bot.id} bot={bot} selected={bot.id === openBot} />
        ))}
      </div>
      <div className="mt-2 flex shrink-0 flex-col items-center">
        <RailLink href="/agents/" label="Agents" selected={pathname.startsWith("/agents")} compact>
          <LayoutGrid className="size-5 text-icon" aria-hidden />
        </RailLink>
        <RailLink href="/bots/new/" label="New bot" selected={pathname.startsWith("/bots/new")} compact>
          <Plus className="size-5 text-icon" aria-hidden />
        </RailLink>
        <RailLink href="/settings/" label="Settings" selected={pathname.startsWith("/settings")} compact>
          <Settings className="size-5 text-icon" aria-hidden />
        </RailLink>
        <span aria-hidden className="mt-3 flex size-8 items-center justify-center rounded-full bg-user-avatar text-[13px] font-semibold text-white">
          {initial}
        </span>
      </div>
    </nav>
  );
}

// The selected item sits on a 52 px square. The bottom items are 44 px apart, so their squares overlap a little.
function RailLink({ href, label, selected, compact = false, children }: { href: string; label: string; selected: boolean; compact?: boolean; children: ReactNode }) {
  return (
    <Link
      href={href}
      aria-label={label}
      title={label}
      aria-current={selected ? "page" : undefined}
      className={cn("flex size-[52px] shrink-0 items-center justify-center rounded-[12px] hover:bg-pill", compact && "-my-1", selected && "bg-selected hover:bg-selected")}
    >
      {children}
    </Link>
  );
}

// State marks (wireframe spec, "Rail and header"): Working is a green dot at the bottom right, Waiting a blue
// dot at the top right, Paused is 50% opacity, Limited is 50% plus an hourglass badge, Needs setup a plug badge.
// A mark's ring takes the color of the square behind it, hovered too.
function BotLink({ bot, selected }: { bot: Bot; selected: boolean }) {
  const dimmed = bot.state === "paused" || bot.state === "limited";
  const ring = selected ? "ring-selected" : "ring-rail group-hover:ring-pill";
  return (
    <Link
      href={`/chat/?bot=${encodeURIComponent(bot.id)}`}
      aria-label={`${bot.name}, ${stateLabel(bot)}`}
      title={bot.name}
      aria-current={selected ? "page" : undefined}
      className={cn("group relative flex size-[52px] shrink-0 items-center justify-center rounded-[12px] hover:bg-pill", selected && "bg-selected hover:bg-selected")}
    >
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className={cn(dimmed && "opacity-50")} />
      {bot.state === "working" && <span className={cn("absolute left-[36px] top-[35px] size-2.5 rounded-full bg-working ring-2", ring)} />}
      {bot.state === "waiting" && <span className={cn("absolute left-[37px] top-[10px] size-2.5 rounded-full bg-accent ring-2", ring)} />}
      {(bot.state === "limited" || bot.state === "needs_setup") && (
        <span className={cn("absolute left-[35px] top-[35px] flex size-4 items-center justify-center rounded-full bg-pill ring-2", ring)}>
          {bot.state === "limited" ? <Hourglass className="size-2.5 text-icon" aria-hidden /> : <Plug className="size-2.5 text-icon" aria-hidden />}
        </span>
      )}
    </Link>
  );
}
