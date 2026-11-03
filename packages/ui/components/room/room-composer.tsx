"use client";

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, AtSign, Users } from "lucide-react";
import { findMentions, type Room } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText, SHORT_STATE, STATE_TEXT } from "@/lib/labels";
import { insertMention, mentionOptions, typedMention, type MentionOption } from "@/lib/mentions";
import { roomBots } from "@/lib/state";
import { useApp } from "@/lib/store";
import { scrollToShow, useRevision, useSendGuard } from "@/lib/use-draft";
import { usePhone } from "@/lib/use-phone";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { useDictation } from "@/lib/use-dictation";
import { DictationButton, DictationOptions, DictationStatus } from "../chat/dictation-button";

// A room's composer (WF5). Typing "@" opens the mention popover above it, filtered by what follows; it offers the room's own bots (members),
// as only they answer there. A post that mentions none of them gets no answer, and the composer says so (WF11); mentions are read as the daemon
// reads them, against every bot's name. Room posts carry no files.
// Whether an Escape should close the open mention list: only while it is open, and not one the box already took.
export const mentionEscape = (event: { key: string; defaultPrevented: boolean }, open: boolean): boolean => open && event.key === "Escape" && !event.defaultPrevented;

// The text after typing "@" at the caret: a space first unless the caret already follows one, so the mention starts a word.
export function withAt(text: string, caret: number): { text: string; caret: number } {
  const before = text.slice(0, caret);
  const at = before && !/\s$/.test(before) ? " @" : "@";
  return { text: `${before}${at}${text.slice(caret)}`, caret: caret + at.length };
}

// What @everyone stands for in the mention list.
export const everyoneLabel = (count: number): string => `All ${count} ${count === 1 ? "bot" : "bots"} in this room`;

export function RoomComposer({ room }: { room: Room }) {
  const phone = usePhone();
  const bots = useApp((state) => state.bots);
  const members = useMemo(() => roomBots(room, bots), [room, bots]);
  const field = useRef<HTMLTextAreaElement>(null);
  const list = useRef<HTMLUListElement>(null);
  // Typing goes on while a post is on its way; its success clears the box only if nothing was typed since.
  const revision = useRevision();
  const guard = useSendGuard();
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [highlight, setHighlight] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [hint, setHint] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const typed = typedMention(text, caret);
  const options = typed && typed.start !== dismissed ? mentionOptions(typed.query, members) : [];
  const open = options.length > 0;
  const active = Math.min(highlight, options.length - 1);

  function edit(next: string, at: number): void {
    revision.bump();
    setText(next);
    setCaret(at);
    setHighlight(0);
    // Escape closes the popover for the mention being typed; a new "@" opens it again, even in the same place.
    if (typedMention(next, at)?.start !== dismissed) setDismissed(null);
  }

  const voice = useDictation("room", room.id, field, text, (next) => edit(next, next.length));

  function pick(option: MentionOption): void {
    if (!typed) return;
    const next = insertMention(text, typed.start, caret, option.name);
    voice.changed(next.text);
    edit(next.text, next.caret);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.caret, next.caret);
    });
  }

  // The @ button starts a mention at the caret, opens the list and keeps the keyboard up.
  function mention(): void {
    const next = withAt(text, field.current?.selectionStart ?? text.length);
    voice.changed(next.text);
    edit(next.text, next.caret);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.caret, next.caret);
    });
  }

  // The highlighted bot stays in view as the arrows move through a long list. Only the list scrolls, never the page, and the box keeps focus.
  // It runs again when the highlighted option itself changes (a new query can keep the index and change the list), not just when the index does.
  const selected = open ? JSON.stringify([typed?.query ?? "", options[active]?.name ?? ""]) : "";
  useEffect(() => {
    const box = list.current;
    const item = selected ? box?.querySelector<HTMLElement>(`#mention-${active}`) : null;
    if (box && item) box.scrollTop = scrollToShow(box.scrollTop, box.clientHeight, item.offsetTop, item.offsetHeight);
  }, [selected, active]);

  async function send(): Promise<void> {
    if (busy || !text.trim() || !guard.begin()) return;
    void voice.cancel();
    const sentText = text;
    const sentRevision = revision.now();
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { roomId: room.id, text: sentText });
      setHint(findMentions(sentText, bots, members).length === 0);
      if (revision.now() === sentRevision) edit("", 0); // what was typed meanwhile, with its caret and mention, stays
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
      guard.end();
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (open) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setHighlight((active + (event.key === "ArrowDown" ? 1 : options.length - 1)) % options.length);
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        pick(options[active]);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDismissed(typed?.start ?? null);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <div
      data-nav-composer
      className="relative shrink-0 px-4 pb-4"
      // Escape that reaches the composer from anywhere in it (Android's Back is sent to the open list itself) closes the mention being typed. The box's
      // own Escape has already done so and marked the key taken, so it isn't done twice.
      onKeyDown={(event) => {
        if (!mentionEscape(event, open)) return;
        event.preventDefault();
        setDismissed(typed?.start ?? null);
      }}
    >
      {open && (
        <ul ref={list} id="mention-list" role="listbox" aria-label="Mention a bot" className="absolute bottom-full left-4 z-10 mb-2 flex max-h-[min(240px,40dvh)] w-[min(280px,calc(100vw-32px))] flex-col gap-1 overflow-y-auto overscroll-contain rounded-[16px] border border-divider bg-popover p-1.5">
          {options.map((option, index) => (
            <li
              key={option.name}
              id={`mention-${index}`}
              role="option"
              aria-selected={index === active}
              onMouseDown={(event) => {
                event.preventDefault();
                pick(option);
              }}
              className={cn("flex h-9 cursor-default items-center gap-2 rounded-[8px] px-2.5 text-[14px] text-text", index === active && "bg-highlight")}
            >
              {option.bot ? <BotAvatar color={option.bot.color} shape={option.bot.shape} size={18} className="shrink-0" /> : <Users className="size-[18px] shrink-0 text-icon" aria-hidden />}
              <span className="min-w-0 flex-1 truncate">{option.bot ? option.name : "@everyone"}</span>
              <span className={cn("shrink-0 text-[12px]", option.bot ? STATE_TEXT[option.bot.state] : "text-secondary")}>
                {option.bot ? SHORT_STATE[option.bot.state] : everyoneLabel(members.length)}
              </span>
            </li>
          ))}
        </ul>
      )}
      {hint && !text && <p className="mb-2 text-center text-[12px] leading-[18px] text-caption">No bot was mentioned, so nobody will reply. Type @ to ask a bot.</p>}
      <DictationStatus voice={voice} />
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
        data-slot="composer"
        className="flex min-w-0 flex-col rounded-[24px] border border-composer-stroke bg-composer p-1 shadow-composer focus-within:border-accent"
      >
        <textarea
          ref={field}
          value={text}
          rows={1}
          role="combobox"
          aria-label={`Message ${room.name}`}
          aria-expanded={open}
          aria-controls={open ? "mention-list" : undefined}
          aria-activedescendant={open ? `mention-${active}` : undefined}
          aria-autocomplete="list"
          placeholder={phone ? `Message ${room.name}` : `Message ${room.name}. Type @ to mention a bot.`}
          onChange={(event) => { voice.changed(event.target.value); edit(event.target.value, event.target.selectionStart); }}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
          className="block max-h-[200px] min-h-11 w-full min-w-0 resize-none bg-transparent px-3 py-2.5 text-[16px] leading-6 text-text outline-none [field-sizing:content] placeholder:text-placeholder max-sm:max-h-[min(200px,25dvh)] sm:text-[14px]"
        />
        <div role="group" aria-label="Message actions" className="flex min-h-11 min-w-0 flex-wrap items-center gap-1">
          <button type="button" aria-label="Mention" title="Mention a bot" onClick={mention} className="flex size-11 shrink-0 touch-manipulation select-none items-center justify-center rounded-full text-icon hover:bg-highlight hover:text-text active:bg-highlight">
            <AtSign className="size-[18px]" aria-hidden />
          </button>
          <DictationButton voice={voice} />
          <DictationOptions voice={voice} />
          <button
            type="submit"
            aria-label="Send"
            disabled={busy || !text.trim()}
            className={cn("ml-auto flex size-11 shrink-0 touch-manipulation select-none items-center justify-center rounded-full enabled:active:opacity-80 disabled:opacity-50", text.trim() ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
          >
            <ArrowUp className="size-5" aria-hidden />
          </button>
        </div>
      </form>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </div>
  );
}
