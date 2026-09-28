import type { Exchange, Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { exchangeCaption, exchangeMeta } from "./exchange";

const now = new Date(2026, 8, 27, 14, 0);
const bots = [
  { id: "b1", name: "Pnet apply" },
  { id: "b2", name: "CV Writer" },
];
const exchange = (extra: Partial<Exchange>): Exchange => ({
  id: "x1",
  fromBotId: "b1",
  toBotId: "b2",
  request: "Tailor the CV for listing PN-48213",
  reply: null,
  status: "waiting",
  createdAt: new Date(2026, 8, 27, 9, 31).toISOString(),
  repliedAt: null,
  ...extra,
});
const file: Message = {
  id: "f1",
  conversationId: "x1",
  author: "b2",
  kind: "file",
  body: "CV-Sabenza-Angular.pdf",
  data: { path: "C:\\bots\\b1\\inbox\\CV-Sabenza-Angular.pdf", name: "CV-Sabenza-Angular.pdf", size: 188_416 },
  status: "done",
  createdAt: now.toISOString(),
};

describe("the exchange view", () => {
  it("says when the exchange was asked and how it ended", () => {
    expect(exchangeMeta(exchange({}), now)).toBe("Today 9:31 AM · Waiting for a reply");
    expect(exchangeMeta(exchange({ status: "replied", reply: "Done.", repliedAt: new Date(2026, 8, 27, 9, 33).toISOString() }), now)).toBe("Today 9:31 AM · Replied after 2 minutes");
    expect(exchangeMeta(exchange({ status: "failed", reply: "it was stopped", repliedAt: new Date(2026, 8, 27, 9, 32).toISOString() }), now)).toBe("Today 9:31 AM · No reply: it was stopped");
  });

  it("says where the answer and its files went", () => {
    const replied = exchange({ status: "replied", reply: "Done." });
    expect(exchangeCaption(replied, [], bots)).toBe("Sent back to Pnet apply.");
    expect(exchangeCaption(replied, [file], bots)).toBe("Sent back to Pnet apply. The file is in Pnet apply's inbox.");
    expect(exchangeCaption(exchange({}), [file], bots)).toBeNull();
  });
});
