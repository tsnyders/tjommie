"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot, Message } from "@tjommie/protocol";
import { botHistory, loadConversation, loadOlder, retryConversation } from "@/lib/connection";
import { handoffStatus } from "@/lib/exchange";
import { AUTONOMY_HINT } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { ExchangeView } from "../exchange/exchange-view";
import { FailedLoad } from "../failed-load";
import { PanelBar } from "../panel-bar";
import { BotPanel } from "./bot-panel";
import { ChatHeader } from "./chat-header";
import { Composer } from "./composer";
import { MessageList } from "./message-list";
import { MessageView, shownMessages } from "./message-view";
import { SetupBanner } from "./setup-banner";

const PANEL_KEY = "tjommie.panel";
const NO_MESSAGES: Message[] = [];

// A bot's chat (WF1 Phase 1): the header, the setup banner, the messages, the composer, and the right panel, which
// shows an open exchange (WF6) or else the bot panel.
export function ChatScreen({ botId }: { botId: string }) {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const bot = useMemo(() => bots.find((candidate) => candidate.id === botId), [bots, botId]);
  const conversation = useApp((state) => state.conversations[botId]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [panel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
  const [exchangeId, setExchangeId] = useState<string | null>(null);
  const section = useRef<HTMLElement>(null);
  const panelButton = useRef<HTMLButtonElement>(null);

  // Loads the newest page when the chat opens, and again when a new connection clears it.
  useEffect(() => {
    if (ready && bot && !conversation) void loadConversation(botId, botHistory(botId));
  }, [ready, bot, conversation, botId]);
  // An unknown or deleted bot has no chat.
  useEffect(() => {
    if (!bot) router.replace("/");
  }, [bot, router]);
  const older = useCallback(() => loadOlder(botId, botHistory(botId)), [botId]);
  const render = useCallback(
    (message: Message) => <MessageView message={message} bot={bot} bots={bots} conversationId={botId} onOpenExchange={setExchangeId} openExchangeId={exchangeId} />,
    [bot, bots, botId, exchangeId],
  );

  if (!bot) return null;
  const loaded = conversation?.status === "ready" ? conversation : null;
  const messages = loaded?.messages ?? NO_MESSAGES;
  const showPanel = (open: boolean): void => {
    localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
    setPanel(open);
  };
  const togglePanel = (): void => {
    if (exchangeId) setExchangeId(null);
    else showPanel(!panel);
  };
  // The view's own x and >> go with it, so focus goes back to the row that opened the exchange, or to the panel
  // button if that row is gone. It doesn't scroll there: the chat may have moved on meanwhile.
  const closeExchange = (id: string): void => {
    const row = section.current?.querySelector<HTMLElement>(`[data-exchange="${CSS.escape(id)}"]`);
    setExchangeId(null);
    (row ?? panelButton.current)?.focus({ preventScroll: true });
  };
  const settings = `/bots/settings/?bot=${encodeURIComponent(bot.id)}`;
  return (
    <div className="flex min-h-0 flex-1">
      <section ref={section} aria-label={`Chat with ${bot.name}`} className="flex min-w-0 flex-1 flex-col">
        <ChatHeader bot={bot} panelOpen={panel || exchangeId !== null} onTogglePanel={togglePanel} panelButton={panelButton} />
        {bot.state === "needs_setup" && <SetupBanner bot={bot} />}
        {conversation?.status === "failed" ? (
          <FailedLoad text="Couldn't load this chat." onRetry={() => void retryConversation(botId, botHistory(botId))} />
        ) : !loaded ? (
          <div className="flex-1" />
        ) : shownMessages(messages).length === 0 ? (
          <EmptyChat bot={bot} />
        ) : (
          <MessageList messages={messages} hasMore={loaded.hasMore} onOlder={older} render={render} />
        )}
        <Composer bot={bot} messages={messages} />
      </section>
      {exchangeId ? (
        <ExchangeView
          key={exchangeId}
          exchangeId={exchangeId}
          refreshKey={handoffStatus(messages, exchangeId)}
          onClose={() => closeExchange(exchangeId)}
          bar={<PanelBar settingsHref={settings} settingsLabel="Bot settings" hideLabel="Hide the panel" onHide={() => closeExchange(exchangeId)} />}
        />
      ) : (
        panel && <BotPanel bot={bot} messages={messages} onHide={() => showPanel(false)} />
      )}
    </div>
  );
}

function EmptyChat({ bot }: { bot: Bot }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 px-8 text-center">
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className="opacity-35" />
      <p className="max-w-[360px] text-[14px] leading-[21px] text-body">
        Say hi to {bot.name}. {AUTONOMY_HINT[bot.autonomy]}
      </p>
    </div>
  );
}
