"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis, FolderOpen, Globe, Hourglass, PanelRight, Pause, Play, Plug, Settings } from "lucide-react";
import type { Bot } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { errorText, stateLabel } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { IconButton } from "../icon-button";
import { MENU_ITEM, MENU_POPUP } from "../menu";

// Windows draws its caption buttons over the window's top-right 138 x 44 px. With the bot panel hidden they sit
// over this header, so its buttons move 154 px in. The chip stays centered, with room for the buttons both sides.
export function ChatHeader({ bot, panelOpen, onTogglePanel }: { bot: Bot; panelOpen: boolean; onTogglePanel: () => void }) {
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
      <div className={cn("flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5", panelOpen ? "max-w-[calc(100%-176px)]" : "max-w-[calc(100%-452px)]")}>
        <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
        <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
        <HeaderState bot={bot} />
      </div>
      <div className={cn("absolute top-3 flex items-center gap-2", panelOpen ? "right-4" : "right-[154px]")}>
        <IconButton label="Bot panel" active={panelOpen} onClick={onTogglePanel}>
          <PanelRight aria-hidden />
        </IconButton>
        <MoreMenu bot={bot} onProblem={setProblem} />
      </div>
      {problem && (
        <p role="alert" className="absolute left-1/2 top-full z-10 -translate-x-1/2 rounded-full bg-popover px-3 py-1.5 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </header>
  );
}

// The state in words. Working is green and Waiting blue, each with a dot; the others are gray with an icon.
export function HeaderState({ bot }: { bot: Bot }) {
  if (bot.state === "idle") return null;
  const label = stateLabel(bot);
  if (bot.state === "working" || bot.state === "waiting") {
    return (
      <span className={cn("flex shrink-0 items-center gap-1.5 text-[13px]", bot.state === "working" ? "text-working" : "text-accent")}>
        <span aria-hidden className="size-1.5 rounded-full bg-current" />
        {label}
      </span>
    );
  }
  const Icon = bot.state === "limited" ? Hourglass : bot.state === "paused" ? Pause : Plug;
  return (
    <span className="flex shrink-0 items-center gap-1.5 text-[13px] text-secondary">
      <Icon className="size-3.5" aria-hidden />
      {label}
    </span>
  );
}

function MoreMenu({ bot, onProblem }: { bot: Bot; onProblem: (problem: string | null) => void }) {
  const router = useRouter();
  async function run(action: () => Promise<unknown>): Promise<void> {
    onProblem(null);
    try {
      await action();
    } catch (error) {
      onProblem(errorText(error));
    }
  }
  return (
    <Menu.Root>
      <Menu.Trigger aria-label="More" title="More" className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text data-[popup-open]:bg-selected">
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            <Menu.Item className={MENU_ITEM} onClick={() => void run(() => daemon().call("bots.pause", { id: bot.id, paused: !bot.paused }))}>
              {bot.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {bot.paused ? "Resume bot" : "Pause bot"}
            </Menu.Item>
            {bot.browser && (
              <Menu.Item className={MENU_ITEM} onClick={() => void run(() => daemon().call("bots.showBrowser", { id: bot.id }))}>
                <Globe aria-hidden />
                Show browser window
              </Menu.Item>
            )}
            <Menu.Item className={MENU_ITEM} onClick={() => void run(async () => desktop()?.openFolder(bot.homeFolder))}>
              <FolderOpen aria-hidden />
              Open home folder
            </Menu.Item>
            <Menu.Item className={MENU_ITEM} onClick={() => router.push(`/bots/settings/?bot=${encodeURIComponent(bot.id)}`)}>
              <Settings aria-hidden />
              Bot settings
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
