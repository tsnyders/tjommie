"use client";

import { useState, type DragEvent, type KeyboardEvent } from "react";
import { ArrowUp, Paperclip, Plus, Square, X } from "lucide-react";
import { ApprovalData, type Bot, type Message } from "@tjommie/protocol";
import { addFiles, fileName, MAX_FILES } from "@/lib/attachments";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { errorText } from "@/lib/labels";
import { cn } from "@/lib/utils";

// Whether the bot has a turn to stop: it's working, or waiting on a tool card in its chat, which holds its turn.
// A bot waiting only on a routine it proposed isn't running anything.
export function canStop(bot: Pick<Bot, "state">, messages: Message[]): boolean {
  if (bot.state === "working") return true;
  return (
    bot.state === "waiting" &&
    messages.some((message) => {
      const card = message.kind === "approval" ? ApprovalData.safeParse(message.data).data : undefined;
      return card?.status === "pending" && card.kind !== "routine";
    })
  );
}

// The message box. Its round button sends (a working bot gets the message right away, as steering), or stops
// a running bot while the field is empty. Files go with a message: chat.send needs text, so Send stays off
// until there is some. Sending is done all day, so nothing here animates.
export function Composer({ bot, messages }: { bot: Bot; messages: Message[] }) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const hasText = text.trim().length > 0;
  const stop = !hasText && canStop(bot, messages);

  function attach(paths: string[]): void {
    const next = addFiles(files, paths);
    if (next) {
      setFiles(next);
      setProblem(null);
    } else {
      setProblem(`A message can carry up to ${MAX_FILES} files.`);
    }
  }

  async function run(action: () => Promise<unknown>, done?: () => void): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      done?.();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  function submit(): void {
    if (busy) return;
    if (hasText) {
      void run(
        () => daemon().call("chat.send", { botId: bot.id, text, files }),
        () => {
          setText("");
          setFiles([]);
        },
      );
    } else if (stop) {
      void run(() => daemon().call("chat.stop", { botId: bot.id }));
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (hasText) submit(); // Enter never stops a bot; Stop takes a click
  }

  async function pick(): Promise<void> {
    const picked = await desktop()?.pickFiles();
    if (picked?.length) attach(picked);
  }

  function onDrop(event: DragEvent<HTMLFormElement>): void {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    const api = desktop();
    if (api) attach(Array.from(event.dataTransfer.files, (file) => api.pathForFile(file)));
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
      // Only file drags are the form's. Accepting a text drag here would stop the field from taking the text.
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes("Files")) event.preventDefault();
      }}
      onDrop={onDrop}
      className="shrink-0 px-4 pb-4"
    >
      {files.length > 0 && (
        <ul aria-label="Attached files" className="mb-2 flex flex-wrap gap-1.5">
          {files.map((path) => (
            <li key={path} className="inline-flex h-7 max-w-[240px] items-center gap-1.5 rounded-full bg-pill pl-2.5 pr-1 text-[12px] text-text">
              <Paperclip className="size-3 shrink-0 text-icon" aria-hidden />
              <span className="truncate">{fileName(path)}</span>
              <button
                type="button"
                aria-label={`Remove ${fileName(path)}`}
                onClick={() => setFiles(files.filter((file) => file !== path))}
                className="flex size-5 shrink-0 items-center justify-center rounded-full text-icon hover:text-text"
              >
                <X className="size-3" aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex items-end gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-2 pr-[5px] shadow-composer">
        <button
          type="button"
          aria-label="Attach files"
          title="Attach files"
          onClick={() => void pick()}
          className="mb-0.5 flex size-7 shrink-0 items-center justify-center rounded-full bg-disabled text-icon hover:text-text"
        >
          <Plus className="size-4" aria-hidden />
        </button>
        {/* The caret shows focus here: WF11 draws the typing composer without a ring. */}
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={onKeyDown}
          rows={1}
          aria-label={`Message ${bot.name}`}
          placeholder={`Message ${bot.name}`}
          className="max-h-[200px] min-h-8 flex-1 resize-none bg-transparent py-[5.5px] text-[14px] leading-[21px] text-text outline-none [field-sizing:content] placeholder:text-placeholder"
        />
        <button
          type="submit"
          disabled={busy || (!hasText && !stop)}
          aria-label={stop ? "Stop" : "Send"}
          title={stop ? "Stop" : "Send"}
          className={cn(
            "flex size-8 shrink-0 items-center justify-center rounded-full",
            hasText || stop ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder",
          )}
        >
          {stop ? <Square className="size-3.5" aria-hidden /> : <ArrowUp className="size-4" aria-hidden />}
        </button>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
