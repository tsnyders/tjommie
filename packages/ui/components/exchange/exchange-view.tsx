"use client";

import Link from "next/link";
import { useEffect, useState, type ReactNode } from "react";
import { ArrowRight, ArrowUp, X } from "lucide-react";
import type { Bot, Exchange, Message } from "@tjommie/protocol";
import { exchangePage, loadConversation, request } from "@/lib/connection";
import { exchangeCaption, exchangeMeta } from "@/lib/exchange";
import { DELETED_BOT, errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar } from "../bot-avatar";
import { MessageView, shownMessages } from "../chat/message-view";
import { IconButton } from "../icon-button";

const NO_MESSAGES: Message[] = [];

function Avatar({ bot, size }: { bot: Bot | undefined; size: number }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={size} className="shrink-0" /> : <MissingAvatar size={size} />;
}

// The exchange view (WF6): who asked whom, the whole exchange, and a box that replies in the Team room with both
// bots mentioned. It opens in the right panel, in place of the bot panel or the Team room's members. rowStatus is
// the exchange's status where it was opened from (a chat's handoff row), which the daemon updates when the
// exchange is answered or fails.
export function ExchangeView({ exchangeId, rowStatus, onClose, bar }: { exchangeId: string; rowStatus?: string; onClose: () => void; bar: ReactNode }) {
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

  // The header's status lives on the exchange, which has no event of its own. It's fetched again whenever the
  // exchange's messages move on, and when rowStatus changes: the exchange is answered or fails at the end of the
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
  }, [exchangeId, progress, rowStatus]);

  const from = exchange ? bots.find((bot) => bot.id === exchange.fromBotId) : undefined;
  const to = exchange ? bots.find((bot) => bot.id === exchange.toBotId) : undefined;
  const caption = exchange ? exchangeCaption(exchange, messages, bots) : null;
  const shown = shownMessages(messages); // a reply that ended blank leaves no empty row
  return (
    <aside aria-label="Exchange" className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      {bar}
      <header className="mx-4 flex shrink-0 items-start gap-3 border-b border-divider pb-3 pt-1">
        <span className="mt-0.5 flex shrink-0 items-center gap-1.5">
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
        <IconButton label="Close the exchange" onClick={onClose}>
          <X aria-hidden />
        </IconButton>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <ol className="flex flex-col gap-2">
          {shown.map((message, index) => (
            <li key={message.id} className="flex max-w-[400px] flex-col items-start gap-2">
              {message.author !== "system" && message.author !== shown[index - 1]?.author && <AuthorRow id={message.author} bots={bots} />}
              <MessageView message={message} bot={undefined} bots={bots} conversationId={exchangeId} />
            </li>
          ))}
        </ol>
        {caption && <p className="mt-3 text-[12px] leading-[18px] text-secondary">{caption}</p>}
      </div>
      <ExchangeComposer bots={[from, to].filter((bot): bot is Bot => bot !== undefined)} />
    </aside>
  );
}

function AuthorRow({ id, bots }: { id: string; bots: Bot[] }) {
  const bot = bots.find((candidate) => candidate.id === id);
  return (
    <p className="flex items-center gap-2 text-[12px] font-semibold leading-[18px] text-secondary">
      <Avatar bot={bot} size={18} />
      {bot?.name ?? DELETED_BOT}
    </p>
  );
}

// Replies in the Team room with both bots already @mentioned (spec 6.2). There are no attachments here.
function ExchangeComposer({ bots }: { bots: Bot[] }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [posted, setPosted] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const canSend = bots.length > 0 && text.trim() !== ""; // with both bots gone there's no one to mention
  async function send(): Promise<void> {
    if (busy || !canSend) return;
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { text: `${bots.map((bot) => `@${bot.name}`).join(" ")} ${text.trim()}` });
      setText("");
      setPosted(true);
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
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
          Posted in the Team room.{" "}
          <Link href="/room/" className="text-accent hover:underline">
            Open room
          </Link>
        </p>
      )}
      <div className="flex items-center gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-2 pr-[5px]">
        {bots.map((bot) => (
          <span key={bot.id} className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-pill px-2.5 text-[12px] text-text">
            <BotAvatar color={bot.color} shape={bot.shape} size={18} />@{bot.name}
          </span>
        ))}
        {/* The caret shows focus here, as in the chat's composer. */}
        <input
          value={text}
          onChange={(event) => {
            setText(event.target.value);
            setPosted(false);
          }}
          aria-label="Reply in Team room"
          placeholder="Reply in Team room"
          className="min-w-0 flex-1 truncate bg-transparent text-[14px] text-text outline-none placeholder:text-placeholder"
        />
        <button
          type="submit"
          aria-label="Send"
          disabled={busy || !canSend}
          className={cn("flex size-8 shrink-0 items-center justify-center rounded-full", canSend ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
        >
          <ArrowUp className="size-4" aria-hidden />
        </button>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
