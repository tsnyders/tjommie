"use client";

import { useId, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { ArrowUp, Paperclip, Plus, Square, X } from "lucide-react";
import { ApprovalData, type Bot, type Message } from "@tjommie/protocol";
import { IconButton } from "@/components/icon-button";
import { addFiles, fileName, MAX_FILES } from "@/lib/attachments";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { localFileActions, pcName, useLocalFileActions } from "@/lib/native-capability";
import { useApp } from "@/lib/store";
import { attachPaths, useRevision, useSendGuard, withoutSent, type Attachment } from "@/lib/use-draft";
import { cn } from "@/lib/utils";
import { useDictation } from "@/lib/use-dictation";
import { DictationButton, DictationOptions, DictationStatus } from "./dictation-button";

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
  const [text, setDraftText] = useState("");
  const [files, setFileState] = useState<Attachment[]>([]);
  const field = useRef<HTMLTextAreaElement>(null);
  // The attachments as they are now, kept in step with every change. A picker that returns after a send cleared its files must add to what is left,
  // not to the list it saw when it opened, so nothing that was sent comes back.
  const latestFiles = useRef<Attachment[]>([]);
  const setFiles = (next: Attachment[]): void => {
    latestFiles.current = next;
    setFileState(next);
  };
  // Typing goes on while a message is on its way, so its success clears only what it sent: see sent() below.
  const revision = useRevision();
  const guard = useSendGuard();
  const ids = useRef(0);
  const setText = (next: string): void => {
    revision.bump();
    setDraftText(next);
  };
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const hasText = text.trim().length > 0;
  const stop = !hasText && canStop(bot, messages);
  // A file goes as its path on this PC, which only this PC's daemon can read. So, as on a phone, attaching is off while the window shows another PC
  // (its files are attached in Tjommie on that PC), and no path of this PC's ever goes with a message to it. The button stays reachable, marked
  // unavailable, and says why: to a screen reader, and on press.
  const host = useApp((state) => state.host);
  const local = useLocalFileActions();
  const elsewhere = `Attach files on ${pcName(host)}`;
  const reason = useId();
  const voice = useDictation("chat", bot.id, field, text, setText);

  function attach(paths: string[]): void {
    const next = attachPaths(latestFiles.current, paths, addFiles, () => (ids.current += 1));
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
      if (!guard.begin()) return;
      void voice.cancel();
      // What this send carries, fixed now. Its success clears the text only if nothing was typed since, and only these files.
      const sentText = text;
      const sentRevision = revision.now();
      const sentFiles = localFileActions() ? files : [];
      void run(
        () => daemon().call("chat.send", { botId: bot.id, text: sentText, files: sentFiles.map((file) => file.path) }),
        () => {
          if (revision.now() === sentRevision) setText("");
          setFiles(
            withoutSent(
              latestFiles.current,
              sentFiles.map((file) => file.id),
            ),
          );
        },
      ).finally(guard.end);
    } else if (stop) {
      void run(() => daemon().call("chat.stop", { botId: bot.id })); // stopping clears nothing
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    if (hasText) submit(); // Enter never stops a bot; Stop takes a click
  }

  async function pick(): Promise<void> {
    const picked = await localFileActions()?.pickFiles();
    if (picked?.length) attach(picked);
  }

  function onDrop(event: DragEvent<HTMLFormElement>): void {
    if (!event.dataTransfer.types.includes("Files")) return;
    event.preventDefault();
    const api = localFileActions();
    if (api) attach(Array.from(event.dataTransfer.files, (file) => api.pathForFile(file)));
    else setProblem(`${elsewhere}.`);
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
      {local && files.length > 0 && (
        <ul aria-label="Attached files" className="mb-2 flex flex-wrap gap-1.5">
          {files.map(({ id, path }) => (
            <li key={id} className="inline-flex min-h-8 max-w-[240px] items-center gap-2 rounded-full bg-pill pl-3 text-[13px] text-text max-sm:min-h-11">
              <Paperclip className="size-3 shrink-0 text-icon" aria-hidden />
              <span className="truncate">{fileName(path)}</span>
              <IconButton
                label={`Remove ${fileName(path)}`}
                onClick={() => setFiles(latestFiles.current.filter((file) => file.id !== id))}
                className="rounded-full max-sm:size-11"
              >
                <X className="size-4" aria-hidden />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      <DictationStatus voice={voice} />
      <div data-slot="composer" className="flex min-w-0 flex-col rounded-[22px] border border-composer-stroke bg-composer p-[5px] shadow-composer focus-within:border-accent">
        <textarea
          ref={field}
          value={text}
          onChange={(event) => { voice.changed(event.target.value); setText(event.target.value); }}
          onKeyDown={onKeyDown}
          rows={1}
          aria-label={`Message ${bot.name}`}
          placeholder={`Message ${bot.name}`}
          className="block max-h-[200px] min-h-8 w-full min-w-0 resize-none bg-transparent px-3 py-2 text-[14px] leading-[21px] text-text outline-none [field-sizing:content] placeholder:text-placeholder max-sm:max-h-[min(200px,25dvh)] max-sm:min-h-11"
        />
        <div role="group" aria-label="Message actions" className="flex min-h-8 min-w-0 flex-wrap items-center gap-1 max-sm:min-h-11">
          <IconButton label="Attach files" aria-disabled={!local} aria-describedby={local ? undefined : reason}
            title={local ? "Attach files" : elsewhere} onClick={() => (local ? void pick() : setProblem(`${elsewhere}.`))}
            className="rounded-full aria-disabled:opacity-50 max-sm:size-11">
            <Plus className="size-[18px]" aria-hidden />
          </IconButton>
          {!local && <span id={reason} className="sr-only">{elsewhere}</span>}
          <DictationButton voice={voice} />
          <DictationOptions voice={voice} />
          <button
            type="submit"
            disabled={busy || (!hasText && !stop)}
            aria-label={stop ? "Stop" : "Send"}
            title={stop ? "Stop" : "Send"}
            className={cn(
              "ml-auto flex size-8 shrink-0 touch-manipulation select-none items-center justify-center rounded-full enabled:active:opacity-80 disabled:opacity-50 max-sm:size-11",
              hasText || stop ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder",
            )}
          >
            {stop ? <Square className="size-4" aria-hidden /> : <ArrowUp className="size-5" aria-hidden />}
          </button>
        </div>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
