"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { PanelRight, RotateCcw, Users } from "lucide-react";
import { TEAM_ROOM_ID, type Message } from "@tjommie/protocol";
import { loadConversation, loadOlder, retryConversation, roomHistory } from "@/lib/connection";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MessageList } from "../chat/message-list";
import { MessageView } from "../chat/message-view";
import { ExchangeView } from "../exchange/exchange-view";
import { IconButton } from "../icon-button";
import { PanelBar } from "../panel-bar";
import { Pill } from "../pill";
import { MembersPanel } from "./members-panel";
import { RoomComposer } from "./room-composer";
import { RoomPost } from "./room-post";

const PANEL_KEY = "tjommie.panel";

// The Team room (WF5): every bot is a member. Text posts get the room's own layout; activity rows, notices and
// errors (with Retry) sit indented under the posts, drawn like in a chat.
export function RoomScreen() {
  const bots = useApp((state) => state.bots);
  const conversation = useApp((state) => state.conversations[TEAM_ROOM_ID]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [panel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
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
    localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
    setPanel(open);
  };
  const togglePanel = (): void => {
    if (exchangeId) setExchangeId(null);
    else showPanel(!panel);
  };
  // As in a chat: the view's own x and >> go with it, so focus goes back to the "Open exchange" that opened it, or
  // to the panel button if that post is gone. It doesn't scroll there: the room may have moved on meanwhile.
  const closeExchange = (): void => {
    setExchangeId(null);
    (opener.current?.isConnected ? opener.current : panelButton.current)?.focus({ preventScroll: true });
  };
  // The room's newest post or notice for the open exchange. Its reply post and its failure notice are saved with
  // the exchange's new status, so the view fetches the exchange again when this changes.
  const exchangeKey = exchangeId ? loaded?.messages.findLast((message) => message.data?.exchangeId === exchangeId)?.id : undefined;
  return (
    <div className="flex min-h-0 flex-1">
      <section aria-label="Team room" className="flex min-w-0 flex-1 flex-col">
        <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
          <div className="flex h-10 items-center gap-2 rounded-full bg-chip px-3.5">
            <Users className="size-[18px] text-icon" aria-hidden />
            <h1 className="text-[15px] font-semibold text-title">Team room</h1>
            <span className="text-[13px] text-secondary">
              {bots.length} {bots.length === 1 ? "bot" : "bots"}
            </span>
          </div>
          <div className={cn("absolute top-3", panelOpen ? "right-4" : "right-[154px]")}>
            <IconButton ref={panelButton} label="Members panel" active={panelOpen} onClick={togglePanel}>
              <PanelRight aria-hidden />
            </IconButton>
          </div>
        </header>
        {conversation?.status === "failed" ? (
          <FailedRoom onRetry={() => void retryConversation(TEAM_ROOM_ID, roomHistory)} />
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
          rowStatus={exchangeKey}
          onClose={closeExchange}
          bar={<PanelBar settingsHref="/settings/?section=bots" settingsLabel="Bots settings" hideLabel="Hide the panel" onHide={closeExchange} />}
        />
      ) : (
        panel && <MembersPanel onHide={() => showPanel(false)} />
      )}
    </div>
  );
}

// The history didn't load, as in a bot's chat. A new connection also loads it again.
function FailedRoom({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center">
      <p className="text-[14px] leading-[21px] text-body">Couldn&apos;t load the Team room.</p>
      <Pill onClick={onRetry}>
        <RotateCcw className="size-3.5" aria-hidden />
        Retry
      </Pill>
    </div>
  );
}
