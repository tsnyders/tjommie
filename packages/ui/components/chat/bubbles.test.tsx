import type { Message } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BotBubble } from "./bubbles";

const reply = (body: string, status: Message["status"]): Message => ({ id: "m1", conversationId: "b1", author: "b1", kind: "text", body, data: null, status, createdAt: "2026-09-27T07:14:00.000Z" });

describe("BotBubble", () => {
  it("stays plain text with a caret while it streams, even before the first text, then renders Markdown", () => {
    // A chat opened mid-reply gets the streaming message with an empty body: it shows the caret, never a blank bubble.
    const opened = renderToStaticMarkup(<BotBubble message={reply("", "streaming")} />);
    expect(opened).toContain('data-author="bot"');
    expect(opened).toContain('aria-hidden="true"');
    expect(renderToStaticMarkup(<BotBubble message={reply("**Done.**", "streaming")} />)).toContain("**Done.**");
    const done = renderToStaticMarkup(<BotBubble message={reply("**Done.**", "done")} />);
    expect(done).toContain("<strong>Done.</strong>");
    expect(done).not.toContain('aria-hidden="true"');
  });

  it("shows nothing for a reply that ended with only whitespace", () => {
    expect(renderToStaticMarkup(<BotBubble message={reply("\n\n", "done")} />)).toBe("");
  });
});
