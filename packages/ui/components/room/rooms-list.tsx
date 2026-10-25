"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import type { Message, Room } from "@tjommie/protocol";
import { daemon, request } from "@/lib/connection";
import { phoneCaptionClear } from "@/lib/native-capability";
import { roomLine } from "@/lib/overview";
import { applyMessageEvent, roomBots, type MessageEvent } from "@/lib/state";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { AvatarStack, STRETCHED } from "../agents/cards";
import { RoomForm } from "./room-form";

const STACK = 5; // avatars on a row before "+N"
const PREVIEW = 10; // each room's newest messages, for its line

// A new or changed message, in the preview of the room it's in. A room not read yet keeps a new post for its page to be merged with; a change to
// one of its messages is in that page, so until it lands the room stays not read yet. A chat's message changes nothing.
export function withMessage(posts: Record<string, Message[]>, event: MessageEvent, rooms: Pick<Room, "id">[]): Record<string, Message[]> {
  const id = event.data.conversationId;
  if (!rooms.some((room) => room.id === id) || (!posts[id] && event.event !== "message.added")) return posts;
  return { ...posts, [id]: applyMessageEvent(posts[id] ?? [], event).slice(-PREVIEW) };
}

// A room's page, merged with what came as events while it was on its way: a message the page doesn't have yet is kept. For a message both have,
// the page's copy wins: it's the daemon's, so a change an event missed (while reconnecting) isn't kept out of date.
export function withPage(posts: Record<string, Message[]>, roomId: string, page: Message[]): Record<string, Message[]> {
  const read = new Set(page.map((message) => message.id));
  const merged = [...page, ...(posts[roomId] ?? []).filter((message) => !read.has(message.id))];
  return { ...posts, [roomId]: merged.sort((a, b) => a.createdAt.localeCompare(b.createdAt)).slice(-PREVIEW) };
}

// Each room's newest messages, for its row here and its card on Agents; a room not read yet has none (undefined). They're read for each list of
// rooms (a room made, changed or deleted, a new connection), one room at a time, and a newer list stops an older read, so what it hadn't asked for
// yet isn't asked. New posts come as events, while a room is read too: bots at work never make it read again (their names are looked up as the
// line is drawn).
export function useRoomPosts(): Record<string, Message[]> {
  const rooms = useApp((state) => state.rooms);
  const ready = useApp((state) => state.connection.status === "ready");
  const [posts, setPosts] = useState<Record<string, Message[]>>({});
  useEffect(() => {
    if (!ready) return;
    let live = true;
    void (async () => {
      for (const room of rooms) {
        if (!live) return;
        const page = await request("room.history", { roomId: room.id, limit: PREVIEW }).catch(() => null);
        if (live && page) setPosts((current) => withPage(current, room.id, page.messages));
      }
    })();
    return () => {
      live = false;
    };
  }, [rooms, ready]);
  useEffect(() => {
    if (!ready) return;
    let off = (): void => {};
    try {
      off = daemon().subscribe((event) => {
        if (event.event === "message.added" || event.event === "message.updated") setPosts((current) => withMessage(current, event, rooms));
      });
    } catch {
      // not connected right now: the next connection subscribes again
    }
    return () => off();
  }, [ready, rooms]);
  return posts;
}

// The rooms list (/room/, where the rail's and the tab's room icon go): Everyone, then the other rooms, oldest first, each with its bots and its
// newest post. New room opens the room form, and the room once it's made. The rows are the Agents list's, so each phone style draws them the same.
export function RoomsList() {
  const rooms = useApp((state) => state.rooms);
  const bots = useApp((state) => state.bots);
  const posts = useRoomPosts();
  const [creating, setCreating] = useState(false);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag shrink-0 px-6 pb-7 pt-12 max-sm:px-4 max-sm:pb-3 max-sm:pt-4">
        <div className={cn("flex items-center justify-between gap-6 max-sm:gap-2", phoneCaptionClear())}>
          <div className="max-sm:min-w-0 max-sm:flex-1">
            <h1 data-slot="title" className="truncate text-[20px] font-semibold leading-[26px] text-title">
              Rooms
            </h1>
            <p data-slot="summary" className="mt-1 text-[13px] leading-[18px] text-secondary">
              {rooms.length} {rooms.length === 1 ? "room" : "rooms"}
            </p>
          </div>
          {/* Drawn as Agents' New bot, which the phone styles restyle by its slot. */}
          <button
            type="button"
            aria-label="New room"
            onClick={() => setCreating(true)}
            data-slot="new-bot"
            className="press inline-flex h-8 items-center gap-1.5 rounded-full bg-user-bubble px-3 text-[14px] font-medium text-user-text max-sm:size-11 max-sm:justify-center max-sm:px-0"
          >
            <Plus className="size-3.5 max-sm:size-5" aria-hidden />
            <span className="max-sm:sr-only">New room</span>
          </button>
        </div>
      </header>
      <div data-nav-clear className="min-h-0 flex-1 overflow-y-auto px-6 pb-6 max-sm:px-4">
        <div data-slot="bot-list" className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4 max-sm:gap-2">
          {rooms.map((room) => {
            const members = roomBots(room, bots);
            return (
              <article key={room.id} data-slot="bot-row" className="relative flex flex-col rounded-[14px] border border-card-stroke bg-card p-4 hover:border-input-stroke">
                <div data-slot="row-head" className="flex items-center gap-2.5">
                  <h2 data-slot="row-name" className="min-w-0 flex-1 truncate text-[15px] font-semibold leading-[20px] text-title">
                    <Link href={`/room/?id=${encodeURIComponent(room.id)}`} className={STRETCHED}>
                      {room.name}
                    </Link>
                  </h2>
                  <AvatarStack bots={members.slice(0, STACK)} />
                  {members.length > STACK && <span className="shrink-0 text-[12px] text-caption">+{members.length - STACK}</span>}
                </div>
                <p data-slot="row-task" className="mt-2.5 truncate text-[13px] leading-[18px] text-body">
                  {roomLine(posts[room.id], bots)}
                </p>
              </article>
            );
          })}
        </div>
      </div>
      {creating && <RoomForm onClose={() => setCreating(false)} />}
    </div>
  );
}
