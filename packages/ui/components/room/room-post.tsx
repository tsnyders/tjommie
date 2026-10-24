import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { ExchangePostData, mentionRanges, type Bot, type Message } from "@tjommie/protocol";
import { clock } from "@/lib/format";
import { DELETED_BOT } from "@/lib/labels";
import { BotAvatar, MissingAvatar } from "../bot-avatar";
import { BotBubble } from "../chat/bubbles";
import { FileCard } from "../chat/file-card";

// A text post in the Team room (WF5). Bots' posts have the avatar, a header line (name, "to <bot>" for bot-to-bot
// posts, time), the bubble, their files and "Open exchange". Your posts are right-aligned, with mentions in blue.
export function RoomPost({ message, bots, onOpenExchange }: { message: Message; bots: Bot[]; onOpenExchange: (exchangeId: string) => void }) {
  if (message.author === "you") return <YourPost text={message.body} bots={bots} />;
  const parsed = ExchangePostData.safeParse(message.data);
  const post = parsed.success ? parsed.data : null;
  const author = bots.find((bot) => bot.id === message.author);
  const to = post ? bots.find((bot) => bot.id === post.toBotId) : undefined;
  return (
    <article className="flex w-full gap-2.5">
      {author ? <BotAvatar color={author.color} shape={author.shape} size={28} className="mt-0.5 shrink-0" /> : <MissingAvatar size={28} />}
      <div className="flex min-w-0 flex-col items-start gap-1.5">
        <p className="flex items-baseline gap-1.5 text-[13px] leading-[18px]">
          <span className="font-semibold text-title">{author?.name ?? DELETED_BOT}</span>
          {post && <span className="text-secondary">to {to?.name ?? DELETED_BOT}</span>}
          <span className="text-[12px] text-caption">{clock(message.createdAt)}</span>
        </p>
        <div className="min-w-0 max-w-[560px]">
          <BotBubble message={message} />
        </div>
        {post?.files?.map((file) => <FileCard key={file.path} file={file} />)}
        {post && (
          <button type="button" onClick={() => onOpenExchange(post.exchangeId)} className="flex items-center gap-1 text-[12px] leading-[18px] text-accent hover:underline">
            Open exchange
            <ChevronRight className="size-3.5" aria-hidden />
          </button>
        )}
      </div>
    </article>
  );
}

function YourPost({ text, bots }: { text: string; bots: Bot[] }) {
  const parts: ReactNode[] = [];
  let at = 0;
  for (const range of mentionRanges(text, bots.map((bot) => bot.name))) {
    if (range.start > at) parts.push(text.slice(at, range.start));
    parts.push(
      <span key={range.start} className="font-semibold text-mention">
        {text.slice(range.start, range.end)}
      </span>,
    );
    at = range.end;
  }
  parts.push(text.slice(at));
  return (
    <div data-author="you" className="max-w-[560px] self-end whitespace-pre-wrap rounded-[18px] bg-user-bubble px-4 py-3 text-[14px] leading-[21px] text-user-text [overflow-wrap:anywhere]">
      {parts}
    </div>
  );
}
