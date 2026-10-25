"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis, PanelRight, Pencil, Settings, Trash2, Users } from "lucide-react";
import type { Message, Room } from "@tjommie/protocol";
import { loadConversation, loadOlder, request, retryConversation, roomHistory } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { captionClear } from "@/lib/native-capability";
import { roomBots } from "@/lib/state";
import { usePhone } from "@/lib/use-phone";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MessageList } from "../chat/message-list";
import { MessageView } from "../chat/message-view";
import { ConfirmDialog } from "../dialog";
import { ExchangeView } from "../exchange/exchange-view";
import { FailedLoad } from "../failed-load";
import { IconButton } from "../icon-button";
import { MENU_ITEM, MENU_POPUP } from "../menu";
import { PanelBar } from "../panel-bar";
import { MembersPanel } from "./members-panel";
import { RoomComposer } from "./room-composer";
import { RoomForm } from "./room-form";
import { RoomPost } from "./room-post";

const PANEL_KEY = "tjommie.panel";

// Where a room's messages are at: a message added, or any of them finishing, moves them on; a reply's words streaming in don't.
export const progressOf = (messages: Message[]): string => messages.map((message) => message.status).join(",");

// A room (WF5): Everyone, which every bot is in, or a room of chosen bots. Text posts get the room's own layout; activity rows, notices and
// errors (with Retry) sit indented under the posts, drawn like in a chat.
export function RoomScreen({ roomId }: { roomId: string }) {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const room = useApp((state) => state.rooms.find((each) => each.id === roomId));
  const members = useMemo(() => (room ? roomBots(room, bots) : []), [room, bots]);
  const conversation = useApp((state) => state.conversations[roomId]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [desktopPanel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
  // On a phone the members are a full-screen sheet, closed to begin with, and never touch the desktop preference.
  const phone = usePhone();
  const [phonePanel, setPhonePanel] = useState(false);
  const panel = phone ? phonePanel : desktopPanel;
  const [exchangeId, setExchangeId] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null); // why Delete room failed
  // A refusal (a bot still answering) is about the room as it was then, so the notice goes once the room moves on.
  const progress = progressOf(conversation?.messages ?? []);
  const [seen, setSeen] = useState(progress);
  if (progress !== seen) {
    setSeen(progress);
    setProblem(null);
  }
  const opener = useRef<HTMLElement | null>(null);
  const panelButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (ready && room && !conversation) void loadConversation(roomId, roomHistory(roomId));
  }, [ready, room, conversation, roomId]);
  // The lists are in before any screen shows, so a room that isn't there is gone: deleted, here or elsewhere, or an old link's.
  useEffect(() => {
    if (!room) router.replace("/room/");
  }, [room, router]);
  const older = useCallback(() => loadOlder(roomId, roomHistory(roomId)), [roomId]);
  // Remembers which "Open exchange" opened the view: each exchange has two in the room, its request and its reply.
  const openExchange = useCallback((id: string) => {
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setExchangeId(id);
  }, []);
  const render = useCallback(
    (message: Message) =>
      message.kind === "text" ? (
        <RoomPost message={message} bots={bots} members={members} onOpenExchange={openExchange} />
      ) : (
        <div className="w-full pl-10">
          <MessageView message={message} bot={undefined} bots={bots} conversationId={roomId} />
        </div>
      ),
    [bots, members, openExchange, roomId],
  );

  if (!room) return null;
  const loaded = conversation?.status === "ready" ? conversation : null;
  const panelOpen = panel || exchangeId !== null;
  const count = `${members.length} ${members.length === 1 ? "bot" : "bots"}`;
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
  // As Delete bot does: the question closes, and a refusal (a bot still answering here) shows under the header. The room.deleted event takes
  // this screen back to the rooms list too.
  const remove = (): void => {
    setDeleting(false);
    setProblem(null);
    request("rooms.delete", { id: room.id }).then(
      () => router.replace("/room/"),
      (error: unknown) => setProblem(errorText(error)),
    );
  };
  // The room's newest post or notice for the open exchange. Its reply post and its failure notice are saved with
  // the exchange's new status, so the view fetches the exchange again when this changes.
  const exchangeKey = exchangeId ? loaded?.messages.findLast((message) => message.data?.exchangeId === exchangeId)?.id : undefined;
  return (
    <div className="relative flex min-h-0 flex-1">
      <section aria-label={room.name} inert={(phone && panelOpen) || undefined} className="flex min-w-0 flex-1 flex-col">
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
                  {room.name}
                </span>
                <span className="shrink-0 text-[13px] font-normal text-secondary">{count}</span>
              </button>
            </h1>
            <MoreMenu room={room} onMembers={() => showPanel(true)} onEdit={() => setEditing(true)} onDelete={() => setDeleting(true)} />
            {problem && (
              <p role="alert" className="absolute left-4 right-4 top-full z-10 rounded-[12px] bg-popover px-3 py-1.5 text-[12px] leading-[18px] text-danger">
                {problem}
              </p>
            )}
          </header>
        ) : (
          <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
            <div className={cn("flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5", panelOpen ? "max-w-[calc(100%-176px)]" : "max-w-[calc(100%-452px)]")}>
              {/* The room's mark sits on a round tile, where a bot's chat shows its 24 px avatar (WF5). */}
              <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-pill">
                <Users className="size-3.5 text-icon" aria-hidden />
              </span>
              <h1 className="truncate text-[15px] font-semibold text-title">{room.name}</h1>
              <span className="shrink-0 text-[13px] text-secondary">{count}</span>
            </div>
            <div className={cn("absolute top-3 flex items-center gap-1", panelOpen ? "right-4" : "right-[154px]")}>
              <IconButton ref={panelButton} label="Members panel" active={panelOpen} onClick={togglePanel}>
                <PanelRight aria-hidden />
              </IconButton>
              {/* Everyone can't be edited or deleted, which is all More would hold here. */}
              {!room.everyone && <MoreMenu room={room} onEdit={() => setEditing(true)} onDelete={() => setDeleting(true)} />}
            </div>
            {problem && (
              <p role="alert" className="absolute left-1/2 top-full z-10 -translate-x-1/2 rounded-full bg-popover px-3 py-1.5 text-[12px] leading-[18px] text-danger">
                {problem}
              </p>
            )}
          </header>
        )}
        {conversation?.status === "failed" ? (
          <FailedLoad text="Couldn't load this room." onRetry={() => void retryConversation(roomId, roomHistory(roomId))} />
        ) : !loaded ? (
          <div className="flex-1" />
        ) : loaded.messages.length === 0 ? (
          <p className="m-auto px-8 text-center text-[13px] leading-[18px] text-secondary">No posts yet. @mention a bot to ask it something.</p>
        ) : (
          <MessageList messages={loaded.messages} hasMore={loaded.hasMore} onOlder={older} render={render} />
        )}
        <RoomComposer room={room} />
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
        panel && <MembersPanel room={room} members={members} onEdit={() => setEditing(true)} onHide={hideMembers} />
      )}
      {editing && <RoomForm room={room} onClose={() => setEditing(false)} />}
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${room.name}?`}
        body="Its posts are deleted. Its bots and their chats aren't touched."
        confirm={
          <>
            <Trash2 className="size-4" aria-hidden />
            Delete room
          </>
        }
        danger
        onConfirm={remove}
      />
    </div>
  );
}

// More, in the room's header. On a phone it leads with Members (onMembers), the sheet the title also opens, and has Bots settings, which the
// sheet's bar shows only on a desktop. Everyone can't be edited or deleted; the other rooms can.
function MoreMenu({ room, onMembers, onEdit, onDelete }: { room: Room; onMembers?: () => void; onEdit: () => void; onDelete: () => void }) {
  const router = useRouter();
  return (
    <Menu.Root>
      <Menu.Trigger aria-label="More" title="More" className="flex size-8 shrink-0 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text data-[popup-open]:bg-selected max-sm:size-11">
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            {onMembers && (
              <Menu.Item className={MENU_ITEM} onClick={onMembers}>
                <PanelRight aria-hidden />
                Members
              </Menu.Item>
            )}
            {!room.everyone && (
              <Menu.Item className={MENU_ITEM} onClick={onEdit}>
                <Pencil aria-hidden />
                Edit room
              </Menu.Item>
            )}
            {onMembers && (
              <Menu.Item className={MENU_ITEM} onClick={() => router.push("/settings/?section=bots")}>
                <Settings aria-hidden />
                Bots settings
              </Menu.Item>
            )}
            {!room.everyone && (
              <Menu.Item className={cn(MENU_ITEM, "text-danger [&_svg]:text-danger")} onClick={onDelete}>
                <Trash2 aria-hidden />
                Delete room
              </Menu.Item>
            )}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}
