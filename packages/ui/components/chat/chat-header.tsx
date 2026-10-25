"use client";

import { useState, type Ref } from "react";
import { useRouter } from "next/navigation";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis, FolderOpen, Globe, Hourglass, PanelRight, Pause, Play, Plug, Settings } from "lucide-react";
import { BackLink } from "../back-link";
import { captionClear, localFileActions, nativeBrowserAvailable } from "@/lib/native-capability";
import type { Bot } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { errorText, stateLabel } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { IconButton } from "../icon-button";
import { MENU_ITEM, MENU_POPUP } from "../menu";

// Windows draws its caption buttons over the window's top-right 138 x 44 px. With the bot panel hidden they sit
// over this header, so its buttons move 154 px in. The chip stays centered, with room for the buttons both sides.
// panelButton reaches the panel button, so the chat can give it focus.
export function ChatHeader({ bot, panelOpen, onTogglePanel, panelButton, onOpenBrowser, phone = false }: { bot: Bot; panelOpen: boolean; onTogglePanel: () => void; panelButton?: Ref<HTMLButtonElement>; onOpenBrowser: () => void; phone?: boolean }) {
  const [problem, setProblem] = useState<string | null>(null);
  // On a phone the header is Back, the bot's title (a button that opens its Details sheet) and More. There is no panel icon, and the right end keeps
  // the caption buttons' room in the desktop window.
  if (phone) {
    return (
      <header className={cn("drag relative flex h-14 shrink-0 items-center gap-1 pl-1", captionClear())}>
        <BackLink href="/agents/" label="Back to Agents" />
        <button
          ref={panelButton}
          type="button"
          aria-expanded={panelOpen}
          aria-label={`${bot.name}, ${stateLabel(bot)}. Show details`}
          onClick={onTogglePanel}
          data-slot="title-chip"
          className="press flex h-11 min-w-0 flex-1 items-center gap-2 overflow-hidden rounded-full bg-chip px-3"
        >
          <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
          {/* Name over state, each cut short: the row is narrow once Back, More and the caption buttons have theirs. */}
          <span className="flex min-w-0 flex-1 flex-col items-start leading-tight">
            <span data-slot="title-name" className="max-w-full truncate text-[15px] font-semibold text-title">{bot.name}</span>
            <HeaderState bot={bot} bounded />
          </span>
        </button>
        <MoreMenu bot={bot} onProblem={setProblem} onOpenBrowser={onOpenBrowser} onDetails={onTogglePanel} />
        {problem && (
          <p role="alert" className="absolute left-4 right-4 top-full z-10 rounded-[12px] bg-popover px-3 py-1.5 text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </header>
    );
  }
  return (
    <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
      <div className={cn("flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5", panelOpen ? "max-w-[calc(100%-176px)]" : "max-w-[calc(100%-452px)]")}>
        <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
        <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
        <HeaderState bot={bot} />
      </div>
      <div className={cn("absolute top-3 flex items-center gap-1", panelOpen ? "right-4" : "right-[154px]")}>
        <IconButton ref={panelButton} label="Bot panel" active={panelOpen} onClick={onTogglePanel}>
          <PanelRight aria-hidden />
        </IconButton>
        <MoreMenu bot={bot} onProblem={setProblem} onOpenBrowser={onOpenBrowser} />
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
export function HeaderState({ bot, bounded = false }: { bot: Bot; bounded?: boolean }) {
  if (bot.state === "idle") return null;
  const label = stateLabel(bot);
  if (bot.state === "working" || bot.state === "waiting") {
    return (
      <span className={cn("flex items-center gap-1.5 text-[13px]", bounded ? "max-w-full min-w-0 text-[12px]" : "shrink-0", bot.state === "working" ? "text-working" : "text-accent")}>
        <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-current" />
        <span className={cn(bounded && "min-w-0 truncate")}>{label}</span>
      </span>
    );
  }
  const Icon = bot.state === "limited" ? Hourglass : bot.state === "paused" ? Pause : Plug;
  return (
    <span className={cn("flex items-center gap-1.5 text-[13px] text-secondary", bounded ? "max-w-full min-w-0 text-[12px]" : "shrink-0")}>
      <Icon className="size-3.5 shrink-0" aria-hidden />
      <span className={cn(bounded && "min-w-0 truncate")}>{label}</span>
    </span>
  );
}

// On a phone the menu leads with Details, the sheet the title also opens.
function MoreMenu({ bot, onProblem, onOpenBrowser, onDetails }: { bot: Bot; onProblem: (problem: string | null) => void; onOpenBrowser: () => void; onDetails?: () => void }) {
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
      <Menu.Trigger aria-label="More" title="More" className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text data-[popup-open]:bg-selected max-sm:size-11">
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            {onDetails && (
              <Menu.Item className={MENU_ITEM} onClick={onDetails}>
                <PanelRight aria-hidden />
                Details
              </Menu.Item>
            )}
            <Menu.Item className={MENU_ITEM} onClick={() => void run(() => daemon().call("bots.pause", { id: bot.id, paused: !bot.paused }))}>
              {bot.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {bot.paused ? "Resume bot" : "Pause bot"}
            </Menu.Item>
            {bot.browser && nativeBrowserAvailable() && (
              <Menu.Item className={MENU_ITEM} onClick={onOpenBrowser}>
                <Globe aria-hidden />
                Open browser
              </Menu.Item>
            )}
            {/* Only the PC app can open a folder. */}
            {localFileActions() && (
              <Menu.Item className={MENU_ITEM} onClick={() => void run(async () => localFileActions()?.openFolder(bot.homeFolder))}>
                <FolderOpen aria-hidden />
                Open home folder
              </Menu.Item>
            )}
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
