import { readFileSync } from "node:fs";
import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { withMessage } from "./rooms-list";

const message = (id: string, conversationId: string, body = "Hi."): Message => ({ id, conversationId, author: "b1", kind: "text", body, data: null, status: "done", createdAt: "2026-10-25T09:00:00.000Z" });

describe("the room previews", () => {
  it("take new and changed messages from events, so they stay current without being read again", () => {
    const posts = { r1: [message("m1", "r1")] };
    const added = withMessage(posts, { type: "event", event: "message.added", data: message("m2", "r1") });
    expect(added.r1.map((each) => each.id)).toEqual(["m1", "m2"]);
    expect(withMessage(added, { type: "event", event: "message.updated", data: message("m1", "r1", "Edited.") }).r1[0].body).toBe("Edited.");
    let many = added;
    for (let index = 3; index <= 12; index++) many = withMessage(many, { type: "event", event: "message.added", data: message(`m${index}`, "r1") });
    expect(many.r1.map((each) => each.id)).toEqual(["m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12"]); // the newest 10
    expect(withMessage(posts, { type: "event", event: "message.added", data: message("m3", "b1") })).toBe(posts); // a chat, or a room not read yet
  });

  it("are read again when the rooms change, not whenever a bot does", () => {
    const source = readFileSync(new URL("./rooms-list.tsx", import.meta.url), "utf8");
    expect(source).toContain("}, [rooms, ready]);");
  });
});
