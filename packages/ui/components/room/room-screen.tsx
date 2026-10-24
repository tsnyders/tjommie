"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis, PanelRight, Settings, Users } from "lucide-react";
import { TEAM_ROOM_ID, type Message } from "@tjommie/protocol";
import { loadConversation, loadOlder, retryConversation, roomHistory } from "@/lib/connection";
import { captionClear } from "@/lib/native-capability";
import { usePhone } from "@/lib/use-phone";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MessageList } from "../chat/message-list";
import { MessageView } from "../chat/message-view";
import { ExchangeView } from "../exchange/exchange-view";
import { FailedLoad } from "../failed-load";
import { IconButton } from "../icon-button";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { PanelBar } from "../panel-bar";
import { MembersPanel } from "./members-panel";
import { RoomComposer } from "./room-composer";
import { RoomPost } from "./room-post";

const PANEL_KEY = "tjommie.panel";

// The Team room (WF5): every bot is a member. Text posts get the room's own layout; activity rows, notices and
// errors (with Retry) sit indented under the posts, drawn like in a chat.
export function RoomScreen() {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const conversation = useApp((state) => state.conversations[TEAM_ROOM_ID]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [desktopPanel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
  // On a phone the members are a full-screen sheet, closed to begin with, and never touch the desktop preference.
  const phone = usePhone();
  const [phonePanel, setPhonePanel] = useState(false);
  const panel = phone ? phonePanel : desktopPanel;
  const [exchangeId, setExchangeId] = useState<string | null>(null);
  const opener = useRef<HTMLElement | null>(null);
  const panelButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (ready && !conversation) void loadConversation(TEAM_ROOM_ID, roomHistory);
  }, [ready, conversation]);
  const older = useCallback(() => loadOlder(TEAM_ROOM_ID, roomHistory), []);
  // Remembers which "Open exchange" opened the view: each exchange has two in the room, its request and its reply.
  const openExchange = useCallback((id: string) => {
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setExchangeId(id);
  }, []);
  const render = useCallback(
    (message: Message) =>
      message.kind === "text" ? (
        <RoomPost message={message} bots={bots} onOpenExchange={openExchange} />
      ) : (
        <div className="w-full pl-10">
          <MessageView message={message} bot={undefined} bots={bots} conversationId={TEAM_ROOM_ID} />
        </div>
      ),
    [bots, openExchange],
  );

  const loaded = conversation?.status === "ready" ? conversation : null;
  const panelOpen = panel || exchangeId !== null;
  const showPanel = (open: boolean): void => {
    if (phone) setPhonePanel(open);
    else {
      localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
      setPanel(open);
    }
  };
  const togglePanel = (): void => {
    if (exchangeId) setExchangeId(null);
    else showPanel(!panel);
  };
  // As in a chat: the view's own x and >> go with it, so focus goes back to the "Open exchange" that opened it, or
  // to the panel button if that post is gone. It doesn't scroll there: the room may have moved on meanwhile.
  const closeExchange = (): void => {
    setExchangeId(null);
    // After the change is drawn: until then the page behind a phone's sheet is still inert and can't take focus.
    requestAnimationFrame(() => (opener.current?.isConnected ? opener.current : panelButton.current)?.focus({ preventScroll: true }));
  };
  const hideMembers = (): void => {
    showPanel(false);
    if (phone) requestAnimationFrame(() => panelButton.current?.focus({ preventScroll: true }));
  };
  // The room's newest post or notice for the open exchange. Its reply post and its failure notice are saved with
  // the exchange's new status, so the view fetches the exchange again when this changes.
  const exchangeKey = exchangeId ? loaded?.messages.findLast((message) => message.data?.exchangeId === exchangeId)?.id : undefined;
  return (
    <div className="relative flex min-h-0 flex-1">
      <section aria-label="Team room" inert={(phone && panelOpen) || undefined} className="flex min-w-0 flex-1 flex-col">
        {phone ? (
          // On a phone, as in a chat: the room's title is a button that opens the members sheet, and More holds Members and Bots settings.
          <header className={cn("drag relative flex h-14 shrink-0 items-center gap-1 pl-3", captionClear())}>
            <h1 className="min-w-0 flex-1">
              <button
                ref={panelButton}
                type="button"
                aria-expanded={panelOpen}
                onClick={togglePanel}
                data-slot="title-chip"
                className="press flex h-11 max-w-full items-center gap-2 rounded-full bg-chip px-3.5"
              >
                <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-pill">
                  <Users className="size-3.5 text-icon" aria-hidden />
                </span>
                <span data-slot="title-name" className="truncate text-[15px] font-semibold text-title">
                  Team room
                </span>
                <span className="shrink-0 text-[13px] font-normal text-secondary">
                  {bots.length} {bots.length === 1 ? "bot" : "bots"}
                </span>
              </button>
            </h1>
            <Menu.Root>
              <Menu.Trigger aria-label="More" title="More" className="flex size-11 shrink-0 items-center justify-center rounded-[10px] text-icon data-[popup-open]:bg-selected">
                <Ellipsis className="size-4" aria-hidden />
              </Menu.Trigger>
              <Menu.Portal>
                <Menu.Positioner side="bottom" align="end" sideOffset={6}>
                  <Menu.Popup className={MENU_POPUP}>
                    <Menu.Item className={MENU_ITEM} onClick={() => showPanel(true)}>
                      <PanelRight aria-hidden />
                      Members
                    </Menu.Item>
                    <Menu.Item className={MENU_ITEM} onClick={() => router.push("/settings/?section=bots")}>
                      <Settings aria-hidden />
                      Bots settings
                    </Menu.Item>
                  </Menu.Popup>
                </Menu.Positioner>
              </Menu.Portal>
            </Menu.Root>
          </header>
        ) : (
          <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
            <div className="flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5">
              {/* The room's mark sits on a round tile, where a bot's chat shows its 24 px avatar (WF5). */}
              <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-pill">
                <Users className="size-3.5 text-icon" aria-hidden />
              </span>
              <h1 className="truncate text-[15px] font-semibold text-title">Team room</h1>
              <span className="shrink-0 text-[13px] text-secondary">
                {bots.length} {bots.length === 1 ? "bot" : "bots"}
              </span>
            </div>
            <div className={cn("absolute top-3", panelOpen ? "right-4" : "right-[154px]")}>
              <IconButton ref={panelButton} label="Members panel" active={panelOpen} onClick={togglePanel}>
                <PanelRight aria-hidden />
              </IconButton>
            </div>
          </header>
        )}
        {conversation?.status === "failed" ? (
          <FailedLoad text="Couldn't load the Team room." onRetry={() => void retryConversation(TEAM_ROOM_ID, roomHistory)} />
        ) : !loaded ? (
          <div className="flex-1" />
        ) : loaded.messages.length === 0 ? (
          <p className="m-auto px-8 text-center text-[13px] leading-[18px] text-secondary">No posts yet. @mention a bot to ask it something.</p>
        ) : (
          <MessageList messages={loaded.messages} hasMore={loaded.hasMore} onOlder={older} render={render} />
        )}
        <RoomComposer />
      </section>
      {exchangeId ? (
        <ExchangeView
          key={exchangeId}
          exchangeId={exchangeId}
          refreshKey={exchangeKey}
          onClose={closeExchange}
          bar={<PanelBar settingsHref="/settings/?section=bots" settingsLabel="Bots settings" hideLabel="Hide the panel" onHide={closeExchange} />}
        />
      ) : (
        panel && <MembersPanel onHide={hideMembers} />
      )}
    </div>
  );
}
