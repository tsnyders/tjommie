import { readFileSync } from "node:fs";
import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { withMessage, withPage } from "./rooms-list";

const message = (id: string, conversationId: string, body = "Hi."): Message => ({ id, conversationId, author: "b1", kind: "text", body, data: null, status: "done", createdAt: "2026-10-25T09:00:00.000Z" });
const rooms = [{ id: "r1" }, { id: "r2" }]; // "b1" is a bot's chat

describe("the room previews", () => {
  it("take new and changed messages from events, so they stay current without being read again", () => {
    const posts = { r1: [message("m1", "r1")] };
    const added = withMessage(posts, { type: "event", event: "message.added", data: message("m2", "r1") }, rooms);
    expect(added.r1.map((each) => each.id)).toEqual(["m1", "m2"]);
    expect(withMessage(added, { type: "event", event: "message.updated", data: message("m1", "r1", "Edited.") }, rooms).r1[0].body).toBe("Edited.");
    let many = added;
    for (let index = 3; index <= 12; index++) many = withMessage(many, { type: "event", event: "message.added", data: message(`m${index}`, "r1") }, rooms);
    expect(many.r1.map((each) => each.id)).toEqual(["m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12"]); // the newest 10
    expect(withMessage(posts, { type: "event", event: "message.added", data: message("m3", "b1") }, rooms)).toBe(posts); // a chat
  });

  it("keep a post for a room not read yet, for its page to be merged with", () => {
    const posts = { r1: [message("m1", "r1")] };
    expect(withMessage(posts, { type: "event", event: "message.added", data: message("m2", "r2") }, rooms).r2?.map((each) => each.id)).toEqual(["m2"]);
    // A change to one of its messages is in its page; until that lands the room stays not read yet, not empty.
    expect(withMessage(posts, { type: "event", event: "message.updated", data: message("m3", "r2") }, rooms)).toBe(posts);
  });

  it("merge a room's page with what came as events while it was on its way", () => {
    const at = (id: string, minute: number): Message => ({ ...message(id, "r1"), createdAt: `2026-10-25T09:${String(minute).padStart(2, "0")}:00.000Z` });
    const page = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((minute) => at(`m${minute}`, minute));
    // m11 was posted after the daemon read the page, m0 is left from an older read, and m5 is an older copy.
    const posts = { r1: [at("m0", 0), { ...at("m5", 5), body: "Old." }, at("m11", 11)], r2: [message("m1", "r2")] };
    const merged = withPage(posts, "r1", page);
    expect(merged.r1.map((each) => each.id)).toEqual(["m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11"]); // the newest 10
    expect(merged.r1.find((each) => each.id === "m5")?.body).toBe("Hi."); // the page's copy: it's the daemon's
    expect(merged.r2).toBe(posts.r2);
  });

  it("are read again when the rooms change, not whenever a bot does", () => {
    const source = readFileSync(new URL("./rooms-list.tsx", import.meta.url), "utf8");
    expect(source).toContain("}, [rooms, ready]);");
  });
});
