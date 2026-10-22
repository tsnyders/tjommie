"use client";

import { useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import Link from "next/link";
import { ChevronsRight, Settings } from "lucide-react";
import type { Bot, Message } from "@tjommie/protocol";
import { clampWidth, PANES, readWidth, saveWidth, splitterWidth, widthBounds, type Pane } from "@/lib/panel-layout";
import { cn } from "@/lib/utils";
import { IconButton } from "../icon-button";
import { BotPanel } from "./bot-panel";
import { BrowserPanel } from "./browser-panel";

// Windows draws its caption buttons over the window's top-right 138 x 44 px, and this panel is always at that edge, so the
// bar's own controls stay at its left and leave 146 px clear at the right, expanded or not.
const CAPTION_CLEAR = "pr-[146px]";

// The right panel beside a bot's chat: a bar (settings, hide, and the Details / Browser switch), the two panes, and a
// separator on its left edge to resize it. The Browser pane is mounted once opened and stays mounted, hidden, while
// Details shows. `overlay` (a narrow window) draws it over the chat; `expanded` over the whole content area.
export function RightPanel({
  bot,
  messages,
  pane,
  onPane,
  expanded,
  onExpandedChange,
  room,
  overlay,
  onHide,
}: {
  bot: Bot;
  messages: Message[];
  pane: Pane;
  onPane: (pane: Pane) => void;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
  room: number;
  overlay: boolean;
  onHide: () => void;
}) {
  const shown: Pane = bot.browser ? pane : "details";
  const [opened, setOpened] = useState(shown === "browser");
  if (shown === "browser" && !opened) setOpened(true);
  const root = useRef<HTMLElement>(null);
  const [widths, setWidths] = useState(() => ({ details: readWidth(localStorage, "details"), browser: readWidth(localStorage, "browser") }));
  const bounds = widthBounds(shown, room);
  const width = clampWidth(widths[shown], bounds);
  const grow = expanded && shown === "browser";
  const setWidth = (next: number): void => {
    const clamped = clampWidth(next, bounds);
    setWidths((all) => ({ ...all, [shown]: clamped }));
    saveWidth(localStorage, shown, clamped);
  };

  const restore = (): void => {
    onExpandedChange(false);
    requestAnimationFrame(() => root.current?.querySelector<HTMLElement>('[data-focus-return="expand"]')?.focus());
  };
  // Escape restores the browser, unless something in it is using Escape: a page dialog, a typed address, or the page itself
  // while the user is in control of it (then the Restore button is the way).
  const escape = (event: KeyboardEvent): void => {
    if (event.key !== "Escape" || !grow || event.defaultPrevented) return;
    if ((event.target as HTMLElement).closest("[data-keeps-escape]") || root.current?.querySelector('[role="alertdialog"]')) return;
    restore();
  };
  // Leaving the browser lets go of it: the pane's own effect releases held keys and hands the browser back to the bot.
  const choose = (next: Pane): void => {
    if (next === "details") {
      onExpandedChange(false);
      // Focus inside the browser would be left on a hidden page; the Details tab is where it belongs.
      if (root.current?.querySelector("#pane-browser")?.contains(document.activeElement)) requestAnimationFrame(() => document.getElementById("tab-details")?.focus());
    }
    onPane(next);
  };

  return (
    <aside
      ref={root}
      aria-label={`About ${bot.name}`}
      onKeyDown={escape}
      style={grow ? undefined : { width }}
      className={cn("flex flex-col border-l border-divider bg-canvas", grow ? "absolute inset-0 z-20" : overlay ? "absolute inset-y-0 right-0 z-20 shadow-[0_0_32px_rgba(0,0,0,0.35)]" : "relative shrink-0")}
    >
      {!grow && <Separator pane={shown} width={width} bounds={bounds} onWidth={setWidth} />}
      <div className={cn("drag flex h-11 shrink-0 items-center gap-1 pl-4", CAPTION_CLEAR)}>
        <Link href={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`} aria-label="Bot settings" title="Bot settings" className="press flex size-8 shrink-0 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text">
          <Settings className="size-4" aria-hidden />
        </Link>
        <IconButton label="Hide panel" onClick={onHide}>
          <ChevronsRight aria-hidden />
        </IconButton>
        {bot.browser && <PaneSwitch pane={shown} onPane={choose} />}
      </div>
      <div id="pane-details" role="tabpanel" aria-labelledby="tab-details" hidden={shown !== "details"} className={cn("min-h-0 flex-1 flex-col", shown === "details" ? "flex" : "hidden")}>
        <BotPanel bot={bot} messages={messages} onOpenBrowser={() => choose("browser")} />
      </div>
      {bot.browser && opened && (
        <div id="pane-browser" role="tabpanel" aria-labelledby="tab-browser" hidden={shown !== "browser"} className={cn("min-h-0 flex-1 flex-col", shown === "browser" ? "flex" : "hidden")}>
          <BrowserPanel bot={bot} active={shown === "browser"} expanded={grow} onToggleExpanded={() => (grow ? restore() : onExpandedChange(true))} />
        </div>
      )}
    </aside>
  );
}

const PANE_LABEL: Record<Pane, string> = { details: "Details", browser: "Browser" };

// Details | Browser, always visible. A tablist: Left and Right move between the two.
function PaneSwitch({ pane, onPane }: { pane: Pane; onPane: (pane: Pane) => void }) {
  const other: Pane = pane === "details" ? "browser" : "details";
  return (
    <div
      role="tablist"
      aria-label="Bot panel views"
      onKeyDown={(event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        onPane(other);
        requestAnimationFrame(() => document.getElementById(`tab-${other}`)?.focus());
      }}
      className="ml-2 flex h-8 items-center gap-0.5 rounded-full bg-pill p-0.5"
    >
      {(["details", "browser"] as const).map((each) => (
        <button
          key={each}
          id={`tab-${each}`}
          type="button"
          role="tab"
          aria-selected={pane === each}
          aria-controls={`pane-${each}`}
          tabIndex={pane === each ? 0 : -1}
          onClick={() => onPane(each)}
          className={cn("press h-7 rounded-full px-3 text-[13px] leading-[18px]", pane === each ? "bg-selected text-text" : "text-secondary hover:text-text")}
        >
          {PANE_LABEL[each]}
        </button>
      ))}
    </div>
  );
}

// The drag handle on the panel's left edge, a window splitter: drag it, or use Left / Right (Shift for bigger steps), Home
// and End. Double-click returns the pane's own default width.
function Separator({ pane, width, bounds, onWidth }: { pane: Pane; width: number; bounds: { min: number; max: number }; onWidth: (width: number) => void }) {
  const drag = useRef<{ x: number; width: number } | null>(null);
  const move = (event: PointerEvent): void => {
    if (drag.current) onWidth(drag.current.width + (drag.current.x - event.clientX));
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize panel"
      aria-valuenow={width}
      aria-valuemin={bounds.min}
      aria-valuemax={bounds.max}
      tabIndex={0}
      onPointerDown={(event) => {
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { x: event.clientX, width };
      }}
      onPointerMove={move}
      onPointerUp={() => (drag.current = null)}
      onPointerCancel={() => (drag.current = null)}
      onDoubleClick={() => onWidth(PANES[pane].fallback)}
      onKeyDown={(event) => {
        const next = splitterWidth(event.key, event.shiftKey, width, bounds);
        if (next === null) return;
        event.preventDefault();
        onWidth(next);
      }}
      className="absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize touch-none outline-none after:absolute after:inset-y-0 after:left-1 after:w-px after:bg-transparent hover:after:bg-accent focus-visible:after:w-0.5 focus-visible:after:bg-accent"
    />
  );
}
