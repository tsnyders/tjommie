import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { timeline } from "@/lib/timeline";
import { shownMessages } from "./message-view";

const message = (id: string, extra: Partial<Message>): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: "", data: null, status: "done", createdAt: "2026-09-27T07:14:00.000Z", ...extra });

describe("shownMessages", () => {
  it("drops bot replies that ended blank, so they leave no time label", () => {
    const blank = message("m1", { body: " \n\n" });
    const typing = message("m2", { status: "streaming" }); // a chat opened mid-reply: the caret shows
    const yours = message("m3", { author: "you", body: "Skip senior roles." });
    const file = message("m4", { kind: "file", data: { path: "C:/cv.pdf", name: "cv.pdf", size: 1 } });
    expect(shownMessages([blank, typing, yours, file]).map((shown) => shown.id)).toEqual(["m2", "m3", "m4"]);
    expect(timeline(shownMessages([blank]))).toEqual([]);
  });
});
