"use client";

import { useEffect, useRef } from "react";

// A composer keeps typing while a send is on its way. These pieces let the send's success clear only what it sent: the text, if nothing was typed since,
// and the files it carried, never ones added meanwhile.

// An attachment with an identity of its own, so a file removed and added again during a send is a new one and stays.
export type Attachment = { id: number; path: string };

// The attachments for a new list of paths: those already attached keep their identity, new ones get the next ids.
export function attachmentsFor(current: Attachment[], paths: string[], nextId: () => number): Attachment[] {
  return paths.map((path) => current.find((each) => each.path === path) ?? { id: nextId(), path });
}

// The attachments after adding files to the current ones, or null when that would be too many (`add` holds the rules: no repeats, at most so many).
export function attachPaths(current: Attachment[], paths: string[], add: (existing: string[], added: string[]) => string[] | null, nextId: () => number): Attachment[] | null {
  const next = add(
    current.map((each) => each.path),
    paths,
  );
  return next ? attachmentsFor(current, next, nextId) : null;
}

// What is left once a send that carried `sent` has gone: everything attached that it didn't carry.
export function withoutSent(current: Attachment[], sent: number[]): Attachment[] {
  return current.filter((each) => !sent.includes(each.id));
}

// Counts edits to a draft. Every change counts, so typing the same text again after erasing it is still a change: a send can tell exactly whether the draft
// is the one it sent.
export function useRevision(): { bump: () => void; now: () => number } {
  const count = useRef(0);
  return {
    bump: () => {
      count.current += 1;
    },
    now: () => count.current,
  };
}

// One send at a time, decided at once: two presses in the same frame can't both start one (the busy state only shows on the next render).
export function useSendGuard(): { begin: () => boolean; end: () => void } {
  const sending = useRef(false);
  return {
    begin: () => {
      if (sending.current) return false;
      sending.current = true;
      return true;
    },
    end: () => {
      sending.current = false;
    },
  };
}

// Whether this component is still the one on screen: a slow save that finishes after it was closed (and maybe replaced) must not act for its successor.
export function useAlive(): { current: boolean } {
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  return alive;
}

// Runs `then` only if the component that started the work is still mounted.
export function ifAlive(alive: { current: boolean }, then: () => void): void {
  if (alive.current) then();
}

// The scroll position that shows an item inside a scrolling list, moving as little as it can: unchanged when the item is already in view.
export function scrollToShow(scrollTop: number, viewHeight: number, itemTop: number, itemHeight: number): number {
  if (itemTop < scrollTop) return itemTop;
  if (itemTop + itemHeight > scrollTop + viewHeight) return itemTop + itemHeight - viewHeight;
  return scrollTop;
}
