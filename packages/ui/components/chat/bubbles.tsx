import { Paperclip } from "lucide-react";
import type { Message } from "@tjommie/protocol";
import { MarkdownText } from "./markdown";

// While the text streams it stays plain, with a caret (alone until the first text arrives); Markdown renders once
// it's done. A reply that ended with only whitespace (the daemon keeps those) shows nothing.
export function BotBubble({ message }: { message: Message }) {
  const streaming = message.status === "streaming";
  if (!streaming && !message.body.trim()) return null;
  return (
    <div data-author="bot" className="w-fit max-w-[600px] rounded-[18px] bg-bubble px-4 py-[14px] text-[14px] leading-[21px] text-text">
      {streaming ? (
        <p className="whitespace-pre-wrap [overflow-wrap:anywhere]">
          {message.body}
          <span aria-hidden className="ml-0.5 inline-block h-4 w-0.5 translate-y-[3px] bg-text" />
        </p>
      ) : (
        <MarkdownText text={message.body} />
      )}
    </div>
  );
}

// The person's own text stays exactly as typed.
export function UserBubble({ message, files }: { message: Message; files: string[] }) {
  return (
    <div className="flex max-w-[600px] flex-col items-end gap-1 self-end">
      {files.length > 0 && (
        <div className="flex flex-wrap justify-end gap-1.5">
          {files.map((name) => (
            <span key={name} className="inline-flex h-7 max-w-[240px] items-center gap-1.5 rounded-full bg-pill px-2.5 text-[12px] text-text">
              <Paperclip className="size-3 shrink-0 text-icon" aria-hidden />
              <span className="truncate">{name}</span>
            </span>
          ))}
        </div>
      )}
      <div data-author="you" className="whitespace-pre-wrap rounded-[18px] bg-user-bubble px-4 py-3 text-[14px] leading-[21px] text-user-text [overflow-wrap:anywhere]">
        {message.body}
      </div>
      {message.status === "queued" && <span className="text-[11px] text-caption">Queued</span>}
    </div>
  );
}
