"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { LayoutGrid, Settings, Users, type LucideIcon } from "lucide-react";
import { agentsTabName, showsTabBar, tabFor, TABS, type TabId } from "@/lib/phone-nav";
import { useApp } from "@/lib/store";
import { useKeyboardVisible } from "@/lib/use-keyboard";
import { useSheetOpen } from "@/lib/use-sheet";
import { cn } from "@/lib/utils";

const ICONS: Record<TabId, LucideIcon> = { agents: LayoutGrid, room: Users, settings: Settings };

// The phone's navigation (below 640 px, in place of the rail): a compact pill floating over the overview screens with Agents, Rooms and
// Settings, each labelled. Nothing backs it: the screens scroll under it, and their scrollers end with room to clear it (data-nav-clear in
// globals.css). One capsule slides to the current tab; the tabs are equal width, so it moves by whole widths. While the keyboard is up the pill
// steps aside, out of the way of what's being typed.
export function TabBar() {
  const pathname = usePathname();
  const waiting = useApp((state) => state.bots.filter((bot) => bot.state === "waiting").length);
  // It steps aside while the keyboard is up, and under a phone sheet (members, an exchange, Details), which has the whole screen.
  const keyboard = useKeyboardVisible();
  const sheet = useSheetOpen();
  if (!showsTabBar(pathname)) return null;
  const hidden = keyboard || sheet;
  const current = tabFor(pathname);
  const index = TABS.findIndex((tab) => tab.id === current);
  return (
    <div
      data-testid="phone-nav"
      data-hidden={hidden || undefined}
      aria-hidden={hidden || undefined}
      inert={hidden}
      className="phone-nav pointer-events-none fixed inset-x-0 z-30 flex justify-center sm:hidden"
    >
      <nav aria-label="Tjommie" className="pointer-events-auto relative flex rounded-full border border-tabbar-stroke bg-tabbar p-[5px] shadow-tabbar">
        {index >= 0 && (
          <span
            aria-hidden
            data-slot="nav-capsule"
            className="absolute bottom-[5px] left-[5px] top-[5px] w-[76px] rounded-full bg-tabbar-selected"
            style={{ transform: `translateX(${index * 100}%)` }}
          />
        )}
        {TABS.map((tab) => {
          const Icon = ICONS[tab.id];
          const selected = current === tab.id;
          return (
            <Link
              key={tab.id}
              href={tab.href}
              aria-current={selected ? "page" : undefined}
              aria-label={tab.id === "agents" ? agentsTabName(waiting) : tab.label}
              className={cn(
                "mobile-motion-choice relative flex h-[50px] w-[76px] flex-col items-center justify-center gap-[3px] rounded-full text-[11px] leading-[14px]",
                selected ? "text-tabbar-selected-fg" : "text-tabbar-fg",
              )}
            >
              <Icon className="size-5" aria-hidden />
              <span>{tab.short}</span>
              {tab.id === "agents" && waiting > 0 && <span aria-hidden className="absolute right-[calc(50%-18px)] top-1.5 size-2.5 rounded-full bg-accent ring-2 ring-tabbar" />}
            </Link>
          );
        })}
      </nav>
    </div>
  );
}
