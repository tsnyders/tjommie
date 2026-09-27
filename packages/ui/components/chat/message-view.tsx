import { MessageData, type Bot, type Message } from "@tjommie/protocol";
import { ActivityRow } from "./activity-row";
import { ApprovalCard } from "./approval-card";
import { BotBubble, UserBubble } from "./bubbles";
import { FileCard } from "./file-card";
import { HandoffRow } from "./handoff-row";
import { ErrorNotice, Notice, RoutineLabel } from "./notices";

// One chat message, drawn by its kind. A payload that doesn't match its schema shows as plain text.
// (safeParse is messageData() without the throw; React's lint rules reject JSX inside try/catch.)
export function MessageView({ message, bot, bots, conversationId }: { message: Message; bot: Bot | undefined; bots: Bot[]; conversationId: string }) {
  const parsed = MessageData.safeParse(message);
  if (!parsed.success) return <Notice text={message.body} />;
  const payload = parsed.data;
  switch (payload.kind) {
    case "text": {
      if (message.author !== "you") return <BotBubble message={message} />;
      const files = payload.data && !("exchangeId" in payload.data) ? payload.data.files : [];
      return <UserBubble message={message} files={files} />;
    }
    case "activity":
      return <ActivityRow summary={message.body} items={payload.data.items} />;
    case "approval":
      return <ApprovalCard summary={message.body} data={payload.data} bot={bot} />;
    case "notice":
      return <Notice text={message.body} />;
    case "error":
      return <ErrorNotice text={message.body} retry={payload.data} />;
    case "handoff":
      return <HandoffRow body={message.body} data={payload.data} bots={bots} conversationId={conversationId} />;
    case "routine":
      return <RoutineLabel text={message.body} data={payload.data} />;
    case "file":
      return <FileCard file={payload.data} />;
  }
}

// A bot reply that ended blank shows nothing (BotBubble hides it too). A list drops these before timeline(), so they
// leave no time label and no empty row.
export const shownMessages = (messages: Message[]): Message[] =>
  messages.filter((message) => message.kind !== "text" || message.author === "you" || message.status === "streaming" || message.body.trim() !== "");
