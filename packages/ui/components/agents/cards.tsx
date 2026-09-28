"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { CircleAlert, Clock, Ellipsis, Hourglass, Loader, MessageCircle, Pause, Play, Plug, Plus, Settings } from "lucide-react";
import { Menu } from "@base-ui/react/menu";
import { ExchangePostData, type Bot, type Message, type Routine } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { botName, DELETED_BOT, errorText, SHORT_STATE, stateLabel } from "@/lib/labels";
import { nextRoutineLine, roomLine, talkingWith, taskLine } from "@/lib/overview";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar, TeamMark } from "../bot-avatar";
import { MENU_ITEM, MENU_POPUP } from "../menu";

const CARD = "relative flex h-[368px] flex-col rounded-[14px] border bg-card p-3 pb-4";
// The footer (WF2): a divider, then a 28 px row 16 px under it.
const FOOTER = "mt-auto flex h-[45px] items-center border-t border-card-stroke pt-4";
const CARD_PILL = "flex h-7 items-center rounded-full px-3 text-[14px] font-medium";
const SLIDE = "transition-transform duration-150 ease-[cubic-bezier(0.23,1,0.32,1)]";
// The card's title is a link stretched over the whole card; the menu sits above it. The footer's pill only shows
// where a click on the card goes, so screen readers skip it and hear the link.
const STRETCHED = "after:absolute after:inset-0 after:rounded-[14px]";
const chatOf = (id: string): string => `/chat/?bot=${encodeURIComponent(id)}`;

// A bot's card (WF2 Phase 1).
export function BotCard({ bot, bots, messages, routines }: { bot: Bot; bots: Bot[]; messages: Message[]; routines: Routine[] }) {
  const [problem, setProblem] = useState<string | null>(null); // why the menu's last action failed
  const waiting = bot.state === "waiting";
  const dim = bot.state === "idle" || bot.state === "needs_setup" ? "opacity-35" : bot.state === "paused" || bot.state === "limited" ? "opacity-50" : undefined;
  const partners = talkingWith(bot.id, messages)
    .map((id) => bots.find((each) => each.id === id))
    .filter((each): each is Bot => each !== undefined);
  return (
    <article className={cn(CARD, "group", waiting ? "border-waiting-stroke" : "border-card-stroke hover:border-input-stroke")}>
      <div className={cn("flex h-[181px] shrink-0 flex-col items-center justify-center gap-2 rounded-[10px] bg-popover leading-[18px]", waiting && "ring-2 ring-inset ring-accent")}>
        <BotAvatar color={bot.color} shape={bot.shape} size={56} className={dim} />
        <PlaceholderState bot={bot} />
      </div>
      <div className="mt-4 flex items-center gap-2.5">
        <BotAvatar color={bot.color} shape={bot.shape} size={28} className="shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-[15px] font-semibold leading-[20px] text-title">
          <Link href={chatOf(bot.id)} className={STRETCHED}>
            {bot.name}
          </Link>
        </h2>
        {/* The chip sits flush right (WF2). The hidden menu button waits over the chip's right end, inside the card; when
            it shows, the chip slides left to make room and the button stays put. Transforms only, so nothing else in
            the row moves. */}
        <span className={cn("relative flex shrink-0 group-hover:-translate-x-9 has-[:focus-visible]:-translate-x-9 has-[[data-popup-open]]:-translate-x-9", SLIDE)}>
          <StateChip bot={bot} />
          <CardMenu bot={bot} onProblem={setProblem} />
        </span>
      </div>
      {/* A failed Pause or Resume says why in place of the task line, as the chat header's menu does under the header. */}
      {problem ? (
        <p role="alert" title={problem} className="mt-2.5 truncate text-[13px] leading-[18px] text-danger">
          {problem}
        </p>
      ) : (
        <p className="mt-2.5 truncate text-[13px] leading-[18px] text-body">{taskLine(bot, messages)}</p>
      )}
      <p className="mt-1.5 flex items-center gap-1.5 text-[12px] leading-[18px] text-secondary">
        <Clock className="size-3.5 shrink-0" aria-hidden />
        <span className="truncate">{nextRoutineLine(bot, routines)}</span>
      </p>
      <div className={cn(FOOTER, "gap-2")}>
        {partners.length > 0 ? (
          <>
            <span className="text-[12px] text-caption">Talking with</span>
            <span className="flex -space-x-1">
              {partners.map((partner) => (
                <BotAvatar key={partner.id} color={partner.color} shape={partner.shape} size={18} />
              ))}
            </span>
          </>
        ) : (
          <span className="text-[12px] text-caption">No bot chats today</span>
        )}
        {waiting && (
          <span aria-hidden className={cn(CARD_PILL, "ml-auto bg-user-bubble text-user-text")}>
            Review
          </span>
        )}
      </div>
    </article>
  );
}

function PlaceholderState({ bot }: { bot: Bot }) {
  if (bot.state === "working")
    return (
      <span className="flex items-center gap-1.5 text-[13px] text-body">
        <Loader className="size-3.5 animate-spin" aria-hidden />
        Working
      </span>
    );
  if (bot.state === "waiting") return <span className="text-[13px] text-accent">Waiting for you</span>;
  if (bot.state === "paused")
    return (
      <span className="flex items-center gap-1.5 text-[13px] text-secondary">
        <Pause className="size-3.5" aria-hidden />
        Paused
      </span>
    );
  if (bot.state === "idle") return <span className="text-[12px] text-placeholder">Idle</span>;
  return <span className="text-[12px] text-secondary">{stateLabel(bot)}</span>;
}

// The chip at the right of the title row (h 22, WF2).
function StateChip({ bot }: { bot: Bot }) {
  const tone = bot.state === "working" ? "bg-pill text-text" : bot.state === "waiting" ? "bg-waiting-fill text-accent" : "bg-popover text-secondary";
  const Icon =
    bot.state === "working" ? Loader : bot.state === "waiting" ? CircleAlert : bot.state === "paused" ? Pause : bot.state === "limited" ? Hourglass : bot.state === "needs_setup" ? Plug : null;
  return (
    <span className={cn("flex h-[22px] shrink-0 items-center gap-1 rounded-full px-2 text-[12px]", tone)}>
      {Icon && <Icon className={cn("size-3", bot.state === "working" && "animate-spin")} aria-hidden />}
      {bot.state === "waiting" ? "Waiting for you" : SHORT_STATE[bot.state]}
    </span>
  );
}

function CardMenu({ bot, onProblem }: { bot: Bot; onProblem: (problem: string | null) => void }) {
  const router = useRouter();
  function pause(): void {
    onProblem(null);
    void request("bots.pause", { id: bot.id, paused: !bot.paused }).catch((error: unknown) => onProblem(errorText(error)));
  }
  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={`Menu for ${bot.name}`}
        className={cn(
          "absolute right-0 top-1/2 z-10 flex size-7 -translate-y-1/2 items-center justify-center rounded-full bg-pill text-icon opacity-0 hover:text-text",
          "focus-visible:translate-x-9 focus-visible:opacity-100 group-hover:translate-x-9 group-hover:opacity-100 data-[popup-open]:translate-x-9 data-[popup-open]:opacity-100",
          SLIDE,
        )}
      >
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            <Menu.Item className={MENU_ITEM} onClick={() => router.push(chatOf(bot.id))}>
              <MessageCircle aria-hidden />
              Open chat
            </Menu.Item>
            <Menu.Item className={MENU_ITEM} onClick={pause}>
              {bot.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {bot.paused ? "Resume bot" : "Pause bot"}
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

// The Team room's card (WF2): the last two bot-to-bot posts, and who talked last.
export function TeamRoomCard({ messages, bots }: { messages: Message[]; bots: Bot[] }) {
  const posts = messages.filter((message) => message.kind === "text" && ExchangePostData.safeParse(message.data).success).slice(-2);
  return (
    <article className={cn(CARD, "border-card-stroke hover:border-input-stroke")}>
      <div className="flex h-[181px] shrink-0 flex-col justify-center gap-3 overflow-hidden rounded-[10px] bg-popover p-3">
        {posts.length === 0 ? (
          <p className="text-center text-[12px] text-placeholder">No bot chats yet</p>
        ) : (
          posts.map((post) => {
            const author = bots.find((each) => each.id === post.author);
            const { toBotId } = ExchangePostData.parse(post.data);
            return (
              <div key={post.id} className="flex gap-2">
                {author ? <BotAvatar color={author.color} shape={author.shape} size={14} className="mt-0.5 shrink-0" /> : <MissingAvatar size={14} />}
                <div className="min-w-0">
                  <p className="text-[11px] leading-[16px] text-caption">
                    <span className="font-semibold text-title">{author?.name ?? DELETED_BOT}</span> to {botName(bots, toBotId)}
                  </p>
                  <p className="line-clamp-3 text-[11px] leading-[16px] text-body">{post.body}</p>
                </div>
              </div>
            );
          })
        )}
      </div>
      <div className="mt-4 flex items-center gap-2.5">
        <TeamMark size={28} />
        <h2 className="text-[15px] font-semibold leading-[20px] text-title">
          <Link href="/room/" className={STRETCHED}>
            Team room
          </Link>
        </h2>
      </div>
      <p className="mt-2.5 truncate text-[13px] leading-[18px] text-body">{roomLine(messages, bots)}</p>
      <div className={cn(FOOTER, "justify-end")}>
        <span aria-hidden className={cn(CARD_PILL, "bg-pill text-text")}>
          Open room
        </span>
      </div>
    </article>
  );
}

export function NewBotCard() {
  return (
    <Link href="/bots/new/" className="flex h-[368px] flex-col items-center justify-center gap-3 rounded-[14px] border border-dashed border-selected text-center hover:bg-card">
      <span className="flex size-12 items-center justify-center rounded-full bg-popover text-icon">
        <Plus className="size-6" aria-hidden />
      </span>
      <span className="text-[15px] font-semibold text-title">New bot</span>
      <span className="text-[12px] text-secondary">Start from a suggestion or from scratch</span>
    </Link>
  );
}
