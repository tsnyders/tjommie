"use client";

import { useEffect, useLayoutEffect, useMemo, useRef } from "react";
import type { Bot, Message } from "@tjommie/protocol";
import { timeline } from "@/lib/timeline";
import { MessageView, shownMessages } from "./message-view";
import { Timestamp } from "./notices";

type Props = { messages: Message[]; hasMore: boolean; bot: Bot; bots: Bot[]; conversationId: string; onOlder: () => Promise<void> };

// Follows the newest message while the person is at the bottom. Near the top it loads the older page, and the
// messages in view stay put: the browser's own scroll anchoring is off, and the list moves by what was added.
export function MessageList({ messages, hasMore, bot, bots, conversationId, onOlder }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const firstShown = useRef<{ id: string; top: number } | null>(null);
  const lastId = useRef<string | undefined>(undefined);
  const shown = useMemo(() => shownMessages(messages), [messages]);
  const items = useMemo(() => timeline(shown), [shown]);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const last = shown.at(-1);
    const justSent = last !== undefined && last.id !== lastId.current && last.author === "you";
    lastId.current = last?.id;
    const before = firstShown.current;
    const kept = before && before.id !== shown[0]?.id ? element.querySelector<HTMLElement>(`[data-message="${before.id}"]`) : null;
    if (before && kept) {
      element.scrollTop += kept.offsetTop - before.top; // an older page was added above
    } else if (pinned.current || justSent) {
      element.scrollTop = element.scrollHeight; // your own message always comes into view
      pinned.current = true;
    }
    const first = element.querySelector<HTMLElement>("[data-message]");
    firstShown.current = first?.dataset.message ? { id: first.dataset.message, top: first.offsetTop } : null;
  }, [shown]);

  // Observes again after each older page (a new first message): an observer only reports changes, so a page too
  // short to push the top out of range would otherwise stop the loading.
  const firstId = shown[0]?.id;
  useEffect(() => {
    const root = scroller.current;
    const target = sentinel.current;
    if (!root || !target || !hasMore) return;
    let loading = false;
    const observer = new IntersectionObserver(
      (entries) => {
        if (loading || !entries.some((entry) => entry.isIntersecting)) return;
        loading = true;
        void onOlder()
          .catch(() => {})
          .finally(() => {
            loading = false;
          });
      },
      { root, rootMargin: "300px 0px 0px 0px" },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [hasMore, onOlder, firstId]);

  return (
    <div
      ref={scroller}
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
      }}
      className="relative min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]"
    >
      <div ref={sentinel} aria-hidden className="h-px" />
      <ol className="flex flex-col gap-3 px-6 pb-4 pt-2">
        {items.map((item) =>
          item.type === "stamp" ? (
            <li key={item.key}>
              <Timestamp label={item.label} />
            </li>
          ) : (
            <li key={item.key} data-message={item.message.id} className="flex flex-col items-start">
              <MessageView message={item.message} bot={bot} bots={bots} conversationId={conversationId} />
            </li>
          ),
        )}
      </ol>
    </div>
  );
}
