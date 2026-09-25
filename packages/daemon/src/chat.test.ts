import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Message } from "@tjommie/protocol";
import { createBot } from "./bots.ts";
import { addMessage, buildRecap, getMessage, history, updateMessage } from "./chat.ts";
import { openDb } from "./db.ts";
import { RpcError } from "./server.ts";
import { tempRoot } from "./testing.ts";

function setup() {
  const root = tempRoot();
  const db = openDb(join(root, "t.db"));
  const bot = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" });
  return { db, bot, root };
}

describe("chat store", () => {
  it("pages through history, newest page first, each page oldest first", () => {
    const { db, bot } = setup();
    const ids = ["one", "two", "three"].map((body) => addMessage(db, { conversationId: bot.id, author: "you", kind: "text", body, status: "done" }).id);
    expect(history(db, bot.id, undefined, 2).map((m) => m.body)).toEqual(["two", "three"]);
    expect(history(db, bot.id, ids[1], 50).map((m) => m.body)).toEqual(["one"]);
  });

  it("rejects a history cursor from another conversation", () => {
    const { db, bot, root } = setup();
    const otherBot = createBot(db, join(root, "bots"), { name: "Reader", color: "green", shape: "pill", instructions: "", provider: "codex", model: "gpt-5.6-sol" });
    addMessage(db, { conversationId: bot.id, author: "you", kind: "text", body: "one", status: "done" });
    const cursor = addMessage(db, { conversationId: otherBot.id, author: "you", kind: "text", body: "other", status: "done" });

    expect.assertions(2);
    try {
      history(db, bot.id, cursor.id, 20);
    } catch (error: unknown) {
      expect(error).toBeInstanceOf(RpcError);
      expect(error).toMatchObject({ code: "not_found", message: "No such message" });
    }
  });

  it("updates body, status and data, and keeps data when an update doesn't mention it", () => {
    const { db, bot } = setup();
    const message = addMessage(db, { conversationId: bot.id, author: bot.id, kind: "text", body: "", data: { files: ["a.pdf"] }, status: "streaming" });
    expect(updateMessage(db, message.id, { body: "Done.", status: "done" })).toMatchObject({ body: "Done.", status: "done", data: { files: ["a.pdf"] } });
    expect(updateMessage(db, message.id, { data: null }).data).toBeNull();
    expect(() => getMessage(db, "missing")).toThrow(/No such message/);
  });

  it("builds a recap from text messages only, oldest first", () => {
    const message = (author: string, kind: Message["kind"], body: string): Message => ({ id: body, conversationId: "c", author, kind, body, data: null, status: "done", createdAt: "" });
    const recap = buildRecap([message("you", "text", "Remember the word ZEBRA."), message("bot-1", "activity", "1 step: Read notes.md"), message("bot-1", "text", "Noted.")], "Writer");
    expect(recap).toContain("Person: Remember the word ZEBRA.\nWriter: Noted.");
    expect(recap).not.toContain("1 step");
    expect(buildRecap([], "Writer")).toBe("");
  });
});
