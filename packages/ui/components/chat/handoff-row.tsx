import Link from "next/link";
import { ArrowRight, ChevronRight, Loader, Users } from "lucide-react";
import type { Bot, HandoffData } from "@tjommie/protocol";
import { DELETED_BOT } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar } from "../bot-avatar";

const ROW = "flex max-w-full items-center gap-2 rounded-[10px] border border-divider px-3 py-2 text-left text-[13px] hover:bg-card";

function Avatar({ bot }: { bot: Bot | undefined }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={18} className="shrink-0" /> : <MissingAvatar size={18} />;
}

// A one-line row for work with another bot (WF1, WF11). An exchange's row opens the exchange in the right panel,
// and it's selected while that exchange is open (WF6); the chat finds it by data-exchange to give it focus back
// when the view closes. A Team room answer's row opens the room.
export function HandoffRow({ body, data, bots, conversationId, onOpen, selected = false }: {
  body: string;
  data: HandoffData;
  bots: Bot[];
  conversationId: string;
  onOpen?: (exchangeId: string) => void;
  selected?: boolean;
}) {
  if ("roomMessageId" in data) {
    return (
      <Link href="/room/" className={ROW}>
        <Users className="size-3.5 shrink-0 text-secondary" aria-hidden />
        <span className="min-w-0 truncate text-body">Answered in the Team room</span>
        <ChevronRight className="size-3.5 shrink-0 text-caption" aria-hidden />
      </Link>
    );
  }
  const from = bots.find((bot) => bot.id === data.fromBotId);
  const to = bots.find((bot) => bot.id === data.toBotId);
  const incoming = conversationId === data.toBotId;
  const fromName = from?.name ?? DELETED_BOT;
  const text = incoming ? `${fromName} asked: ${body}` : `${fromName} asked ${to?.name ?? DELETED_BOT}: ${body}`;
  return (
    <button
      type="button"
      data-exchange={data.exchangeId}
      aria-pressed={selected}
      onClick={() => onOpen?.(data.exchangeId)}
      className={cn(ROW, selected && "border-selected bg-card")}
    >
      <Avatar bot={from} />
      {!incoming && (
        <>
          <ArrowRight className="size-3 shrink-0 text-secondary" aria-hidden />
          <Avatar bot={to} />
        </>
      )}
      <span className="min-w-0 truncate text-body">{text}</span>
      <span className="flex shrink-0 items-center gap-1 text-caption">
        {data.status === "waiting" ? (
          <>
            <Loader className="size-3 animate-spin text-icon" aria-hidden />
            waiting for a reply
          </>
        ) : data.status === "failed" ? (
          "no reply"
        ) : (
          `${data.messages} ${data.messages === 1 ? "message" : "messages"}`
        )}
      </span>
      <ChevronRight className="size-3.5 shrink-0 text-caption" aria-hidden />
    </button>
  );
}
