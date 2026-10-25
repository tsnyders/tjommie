import Link from "next/link";
import { Info } from "lucide-react";
import type { Bot, Room } from "@tjommie/protocol";
import { stateLabel, STATE_TEXT } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { usePhone } from "@/lib/use-phone";
import { useSheet } from "@/lib/use-sheet";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { PanelBar } from "../panel-bar";

// A room's right panel (WF5): its bots with their state in words, and the chain limit with a way to change it. Everyone holds every bot; another
// room's members can be changed here (onEdit opens the room form).
export function MembersPanel({ room, members, onEdit, onHide }: { room: Room; members: Bot[]; onEdit: () => void; onHide: () => void }) {
  const phone = usePhone();
  const { ref: sheetRef, onKeyDown: sheetKeys } = useSheet<HTMLElement>(phone, onHide, '[aria-label="Hide the members"]');
  const settings = useApp((state) => state.settings);
  return (
    <aside ref={sheetRef} onKeyDown={sheetKeys} aria-label={`${room.name} members`} className="mobile-motion-sheet flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas max-sm:absolute max-sm:inset-0 max-sm:z-20 max-sm:w-auto max-sm:border-l-0">
      <PanelBar settingsHref="/settings/?section=bots" settingsLabel="Bots settings" hideLabel="Hide the members" onHide={onHide} />
      <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
        <div className="mt-4 flex items-center justify-between gap-3">
          <h2 className="text-[13px] font-semibold leading-[18px] text-label">Members</h2>
          {!room.everyone && (
            <button type="button" onClick={onEdit} className="text-[13px] leading-[18px] text-accent hover:underline max-sm:min-h-11">
              Edit members
            </button>
          )}
        </div>
        <ul className="-mx-2 flex flex-col">
          {members.map((bot) => (
            <li key={bot.id}>
              <Link href={`/chat/?bot=${encodeURIComponent(bot.id)}`} className="flex h-11 items-center gap-3 rounded-[10px] px-2 hover:bg-card">
                <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
                <span className="min-w-0 flex-1 truncate text-[14px] text-text">{bot.name}</span>
                <span className={cn("shrink-0 text-[12px]", STATE_TEXT[bot.state])}>{stateLabel(bot)}</span>
              </Link>
            </li>
          ))}
        </ul>
        {settings && (
          <p className="mt-6 flex items-center gap-2 text-[12px] leading-[18px] text-secondary">
            <Info className="size-3.5 shrink-0" aria-hidden />
            Bots can ask each other up to {settings.chainLimit} {settings.chainLimit === 1 ? "time" : "times"} per chain.
            <Link href="/settings/?section=bots" className="text-accent hover:underline">
              Change
            </Link>
          </p>
        )}
      </div>
    </aside>
  );
}
