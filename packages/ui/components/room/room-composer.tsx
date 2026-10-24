"use client";

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, AtSign, Users } from "lucide-react";
import { findMentions } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText, SHORT_STATE, STATE_TEXT } from "@/lib/labels";
import { insertMention, mentionOptions, typedMention, type MentionOption } from "@/lib/mentions";
import { useApp } from "@/lib/store";
import { scrollToShow, useRevision, useSendGuard } from "@/lib/use-draft";
import { usePhone } from "@/lib/use-phone";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";

// The Team room's composer (WF5). Typing "@" opens the mention popover above it, filtered by what follows. A post
// that mentions nobody gets no answer, and the composer says so (WF11). Room posts carry no files.
// Whether an Escape should close the open mention list: only while it is open, and not one the box already took.
export const mentionEscape = (event: { key: string; defaultPrevented: boolean }, open: boolean): boolean => open && event.key === "Escape" && !event.defaultPrevented;

// The text after typing "@" at the caret: a space first unless the caret already follows one, so the mention starts a word.
export function withAt(text: string, caret: number): { text: string; caret: number } {
  const before = text.slice(0, caret);
  const at = before && !/\s$/.test(before) ? " @" : "@";
  return { text: `${before}${at}${text.slice(caret)}`, caret: caret + at.length };
}

export function RoomComposer() {
  const phone = usePhone();
  const bots = useApp((state) => state.bots);
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
  const options = typed && typed.start !== dismissed ? mentionOptions(typed.query, bots) : [];
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

  function pick(option: MentionOption): void {
    if (!typed) return;
    const next = insertMention(text, typed.start, caret, option.name);
    edit(next.text, next.caret);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.caret, next.caret);
    });
  }

  // The phone's @ button: starts a mention at the caret, which opens the list, and keeps the keyboard up.
  function mention(): void {
    const next = withAt(text, field.current?.selectionStart ?? text.length);
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
    const sentText = text;
    const sentRevision = revision.now();
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { text: sentText });
      setHint(findMentions(sentText, bots).length === 0);
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
                {option.bot ? SHORT_STATE[option.bot.state] : `All ${bots.length} bots`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {hint && !text && <p className="mb-2 text-center text-[12px] leading-[18px] text-caption">No bot was mentioned, so nobody will reply. Type @ to ask a bot.</p>}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
        data-slot="composer"
        className="flex items-end gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-4 pr-[5px] shadow-composer max-sm:pl-[5px]"
      >
        {phone && (
          <button type="button" aria-label="Mention" onClick={mention} className="flex size-8 shrink-0 items-center justify-center rounded-full bg-disabled text-icon">
            <AtSign className="size-4" aria-hidden />
          </button>
        )}
        {/* The caret shows focus here, as in the chat's composer. */}
        <textarea
          ref={field}
          value={text}
          rows={1}
          role="combobox"
          aria-label="Message the Team room"
          aria-expanded={open}
          aria-controls={open ? "mention-list" : undefined}
          aria-activedescendant={open ? `mention-${active}` : undefined}
          aria-autocomplete="list"
          placeholder={phone ? "Message the Team room" : "Message the Team room. Type @ to mention a bot."}
          onChange={(event) => edit(event.target.value, event.target.selectionStart)}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
          className="max-h-[200px] min-h-8 flex-1 resize-none max-sm:max-h-[min(200px,25dvh)] bg-transparent py-[5.5px] text-[14px] leading-[21px] text-text outline-none [field-sizing:content] placeholder:text-placeholder"
        />
        <button
          type="submit"
          aria-label="Send"
          disabled={busy || !text.trim()}
          className={cn("flex size-8 shrink-0 items-center justify-center rounded-full", text.trim() ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
        >
          <ArrowUp className="size-4" aria-hidden />
        </button>
      </form>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </div>
  );
}
