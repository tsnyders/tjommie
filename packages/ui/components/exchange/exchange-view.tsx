"use client";

import Link from "next/link";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowRight, ArrowUp, X } from "lucide-react";
import { TEAM_ROOM_ID, type Bot, type Exchange, type Message } from "@tjommie/protocol";
import { closeConversation, exchangePage, loadConversation, request, retryConversation } from "@/lib/connection";
import { exchangeCaption, exchangeMeta } from "@/lib/exchange";
import { DELETED_BOT, errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { useSheet } from "@/lib/use-sheet";
import { useRevision, useSendGuard } from "@/lib/use-draft";
import { usePhone } from "@/lib/use-phone";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar } from "../bot-avatar";
import { MessageView, shownMessages } from "../chat/message-view";
import { FailedLoad } from "../failed-load";
import { IconButton } from "../icon-button";
import { useDictation } from "@/lib/use-dictation";
import { DictationButton, DictationOptions, DictationStatus } from "../chat/dictation-button";

const NO_MESSAGES: Message[] = [];

function Avatar({ bot, size }: { bot: Bot | undefined; size: number }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={size} className="shrink-0" /> : <MissingAvatar size={size} />;
}

// The exchange view (WF6): who asked whom, the whole exchange, and a box that replies in Everyone with both bots
// mentioned. It opens in the right panel, in place of the bot panel or a room's members. refreshKey changes when
// the exchange is answered or fails, in the place it was opened from: a chat's handoff row status, or the room's
// newest post or notice for the exchange.
export function ExchangeView({ exchangeId, refreshKey, onClose, bar }: { exchangeId: string; refreshKey?: string; onClose: () => void; bar: ReactNode }) {
  const bots = useApp((state) => state.bots);
  const conversation = useApp((state) => state.conversations[exchangeId]);
  const ready = useApp((state) => state.connection.status === "ready");
  const [exchange, setExchange] = useState<Exchange | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const messages = conversation?.status === "ready" ? conversation.messages : NO_MESSAGES;
  const progress = `${messages.length}:${messages.at(-1)?.status ?? ""}`;

  useEffect(() => {
    if (ready && !conversation) void loadConversation(exchangeId, exchangePage(exchangeId));
  }, [ready, conversation, exchangeId]);
  // Closing the view (or opening another exchange in its place) forgets this one.
  useEffect(() => () => closeConversation(exchangeId), [exchangeId]);

  // The header's status lives on the exchange, which has no event of its own. It's fetched again whenever the
  // exchange's messages move on, and when refreshKey changes: the exchange is answered or fails at the end of the
  // asked bot's turn, after its last message.
  useEffect(() => {
    let live = true;
    request("exchanges.get", { id: exchangeId }).then(
      (page) => {
        if (live) setExchange(page.exchange);
      },
      (error: unknown) => {
        if (live) setProblem(errorText(error));
      },
    );
    return () => {
      live = false;
    };
  }, [exchangeId, progress, refreshKey]);

  const phone = usePhone();
  const { ref: sheetRef, onKeyDown: sheetKeys } = useSheet<HTMLElement>(phone, onClose, '[aria-label="Close the exchange"]');
  const from = exchange ? bots.find((bot) => bot.id === exchange.fromBotId) : undefined;
  const to = exchange ? bots.find((bot) => bot.id === exchange.toBotId) : undefined;
  const caption = exchange ? exchangeCaption(exchange, messages, bots) : null;
  const shown = shownMessages(messages); // a reply that ended blank leaves no empty row
  return (
    <aside ref={sheetRef} onKeyDown={sheetKeys} aria-label="Exchange" className="mobile-motion-sheet flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas max-sm:absolute max-sm:inset-0 max-sm:z-20 max-sm:w-auto max-sm:border-l-0">
      {bar}
      <header className="mx-4 flex shrink-0 items-start gap-3 border-b border-divider pb-2.5 pt-3">
        <span className="mt-1 flex shrink-0 items-center gap-1.5">
          <Avatar bot={from} size={24} />
          <ArrowRight className="size-3.5 text-caption" aria-hidden />
          <Avatar bot={to} size={24} />
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[15px] font-semibold leading-[20px] text-title">
            {exchange ? `${from?.name ?? DELETED_BOT} asked ${to?.name ?? DELETED_BOT}` : "Exchange"}
          </h2>
          <p className="text-[12px] leading-[18px] text-caption">{exchange ? exchangeMeta(exchange) : (problem ?? "Loading")}</p>
        </div>
        {/* The x lines up with the panel's right edge, as WF6 draws it. */}
        <IconButton label="Close the exchange" onClick={onClose} className="-mr-2 -mt-0.5">
          <X aria-hidden />
        </IconButton>
      </header>
      {conversation?.status === "failed" ? (
        <FailedLoad text="Couldn't load this exchange." onRetry={() => void retryConversation(exchangeId, exchangePage(exchangeId))} />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-3 pt-4">
          <ol className="flex flex-col gap-3">
            {shown.map((message, index) => (
              <li key={message.id} className="flex max-w-[400px] flex-col items-start gap-3">
                {message.author !== "system" && message.author !== shown[index - 1]?.author && <AuthorRow id={message.author} bots={bots} />}
                <MessageView message={message} bot={undefined} bots={bots} conversationId={exchangeId} />
              </li>
            ))}
          </ol>
          {caption && <p className="mt-3 text-[12px] leading-[18px] text-secondary">{caption}</p>}
        </div>
      )}
      <ExchangeComposer exchangeId={exchangeId} bots={[from, to].filter((bot): bot is Bot => bot !== undefined)} />
    </aside>
  );
}

function AuthorRow({ id, bots }: { id: string; bots: Bot[] }) {
  const bot = bots.find((candidate) => candidate.id === id);
  return (
    <p className="flex min-w-0 max-w-full items-center gap-2 text-[12px] font-semibold leading-[18px] text-secondary">
      <Avatar bot={bot} size={18} />
      <span className="min-w-0 truncate">{bot?.name ?? DELETED_BOT}</span>
    </p>
  );
}

// Replies in Everyone, wherever the exchange was opened, with both bots already @mentioned (spec 6.2), so both
// answer there. There are no attachments here.
export function ExchangeComposer({ bots, exchangeId }: { bots: Bot[]; exchangeId: string }) {
  const [text, setText] = useState("");
  const field = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [posted, setPosted] = useState(false);
  // Typing goes on while a reply is on its way; its success clears the box only if nothing was typed since.
  const revision = useRevision();
  const guard = useSendGuard();
  const [problem, setProblem] = useState<string | null>(null);
  const change = (next: string): void => { revision.bump(); setText(next); };
  const voice = useDictation("exchange", exchangeId, field, text, change);
  const canSend = bots.length > 0 && text.trim() !== ""; // with both bots gone there's no one to mention
  const asked = bots.length === 2 ? "Ask both bots in Everyone" : bots.length === 1 ? `Ask ${bots[0].name} in Everyone` : "Reply in Everyone";
  async function send(): Promise<void> {
    if (busy || !canSend || !guard.begin()) return;
    void voice.cancel();
    const sentText = text;
    const sentRevision = revision.now();
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { text: `${bots.map((bot) => `@${bot.name}`).join(" ")} ${sentText.trim()}` });
      if (revision.now() === sentRevision) {
        setText("");
        setPosted(true);
      }
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
      guard.end();
    }
  }
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
      className="shrink-0 px-4 pb-4"
    >
      {posted && (
        <p className="mb-2 text-[12px] leading-[18px] text-secondary">
          Posted in Everyone.{" "}
          <Link href={`/room/?id=${TEAM_ROOM_ID}`} className="text-accent hover:underline">
            Open room
          </Link>
        </p>
      )}
      <DictationStatus voice={voice} />
      <div data-slot="composer" className="flex min-w-0 flex-col rounded-[22px] border border-composer-stroke bg-composer p-[5px] shadow-composer focus-within:border-accent">
        {/* Recipients keep their own wrapping row, leaving the full field width for the reply. */}
        <div aria-label="Reply recipients" className="flex min-w-0 flex-wrap gap-2 px-3 pt-2">
          {bots.map((bot) => (
            <span key={bot.id} title={`@${bot.name}`} className="inline-flex min-h-7 min-w-0 max-w-full items-center gap-1.5 rounded-full bg-pill px-2.5 py-1 text-[12px] leading-5 text-text">
              <BotAvatar color={bot.color} shape={bot.shape} size={18} className="shrink-0" />
              <span className="min-w-0 truncate">@{bot.name}</span>
            </span>
          ))}
        </div>
        <input
          ref={field}
          value={text}
          onChange={(event) => {
            voice.changed(event.target.value);
            change(event.target.value);
            setPosted(false);
          }}
          aria-label="Reply in Everyone"
          placeholder={asked}
          className="block min-h-8 w-full min-w-0 bg-transparent px-3 py-2 text-[14px] leading-[21px] text-text outline-none placeholder:text-placeholder max-sm:min-h-11"
        />
        <div role="group" aria-label="Message actions" className="flex min-h-8 min-w-0 flex-wrap items-center gap-1 max-sm:min-h-11">
          <DictationButton voice={voice} />
          <DictationOptions voice={voice} />
          <button
            type="submit"
            aria-label="Send"
            disabled={busy || !canSend}
            className={cn("ml-auto flex size-8 shrink-0 touch-manipulation select-none items-center justify-center rounded-full enabled:active:opacity-80 disabled:opacity-50 max-sm:size-11", canSend ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
          >
            <ArrowUp className="size-5" aria-hidden />
          </button>
        </div>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
