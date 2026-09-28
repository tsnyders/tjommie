"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { ChatScreen } from "@/components/chat/chat-screen";

function Chat() {
  const botId = useSearchParams().get("bot") ?? "";
  // key: each bot's chat starts fresh, with its own draft and attachments.
  return <ChatScreen key={botId} botId={botId} />;
}

// A static export can't have /chat/<id>, so the bot comes as ?bot=<id>. useSearchParams needs the Suspense.
export default function ChatPage() {
  return (
    <Suspense>
      <Chat />
    </Suspense>
  );
}
