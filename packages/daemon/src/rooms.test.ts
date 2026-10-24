import { rmSync } from "node:fs";
import { join } from "node:path";
import { TEAM_ROOM_ID } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createBot, deleteBot, type StoredBot } from "./bots.ts";
import { addMessage } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue } from "./queue.ts";
import { createRoom, deleteRoom, getRoom, listRooms, roomMembers, updateRoom } from "./rooms.ts";
import { tempRoot } from "./testing.ts";

let root: string;
let db: Db;
let alice: StoredBot;
let bob: StoredBot;

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "rooms.db"));
  const makeBot = (name: string) => createBot(db, join(root, "bots"), { name, instructions: "", color: "blue", shape: "circle", provider: "claude", model: "sonnet" });
  alice = makeBot("Alice");
  bob = makeBot("Bob");
});

afterEach(() => {
  db.close();
  rmSync(root, { recursive: true, force: true });
});

describe("rooms", () => {
  it("creates, lists, updates, and deletes custom rooms while Everyone contains all bots", () => {
    const first = createRoom(db, { name: "Planning", botIds: [alice.id, bob.id] });
    const second = createRoom(db, { name: "Review", botIds: [bob.id] });
    expect(first).toMatchObject({ name: "Planning", botIds: [alice.id, bob.id], everyone: false });
    expect(first.createdAt).toMatch(/^\d{4}-\d\d-\d\dT/);
    expect(listRooms(db).map((room) => room.id)).toEqual([TEAM_ROOM_ID, first.id, second.id]);
    expect(getRoom(db, TEAM_ROOM_ID)).toEqual({ id: TEAM_ROOM_ID, name: "Everyone", botIds: [alice.id, bob.id], everyone: true, createdAt: null });
    expect(roomMembers(db, first.id).map((bot) => bot.id)).toEqual([alice.id, bob.id]);
    expect(updateRoom(db, first.id, { name: "Plans", botIds: [bob.id, bob.id] })).toMatchObject({ name: "Plans", botIds: [bob.id] });
    expect(roomMembers(db, first.id).map((bot) => bot.id)).toEqual([bob.id]);
    addMessage(db, { conversationId: first.id, author: "you", kind: "text", body: "Stored note", status: "done" });
    deleteRoom(db, first.id);
    expect(db.prepare("SELECT COUNT(*) AS n FROM messages WHERE conversation_id = ?").get(first.id)).toEqual({ n: 0 });
    expect(() => getRoom(db, first.id)).toThrow(/No such room/);
    expect(listRooms(db).map((room) => room.id)).toEqual([TEAM_ROOM_ID, second.id]);
  });

  it("rejects unknown members, duplicate names including Everyone, and edits to Everyone", () => {
    expect(() => createRoom(db, { name: "Missing", botIds: ["missing"] })).toThrow(expect.objectContaining({ code: "bad_params" }));
    expect(() => createRoom(db, { name: "Empty id", botIds: [""] })).toThrow(expect.objectContaining({ code: "bad_params" }));
    const room = createRoom(db, { name: "Planning", botIds: [alice.id] });
    expect(createRoom(db, { name: "Duplicates", botIds: [alice.id, alice.id] }).botIds).toEqual([alice.id]);
    expect(() => createRoom(db, { name: "pLaNnInG", botIds: [bob.id] })).toThrow(expect.objectContaining({ code: "name_taken" }));
    expect(() => createRoom(db, { name: "everyone", botIds: [bob.id] })).toThrow(expect.objectContaining({ code: "name_taken" }));
    expect(() => updateRoom(db, room.id, { botIds: ["missing"] })).toThrow(expect.objectContaining({ code: "bad_params" }));
    expect(() => updateRoom(db, TEAM_ROOM_ID, { name: "Other" })).toThrow(expect.objectContaining({ code: "bad_state" }));
    expect(() => deleteRoom(db, TEAM_ROOM_ID)).toThrow(expect.objectContaining({ code: "bad_state" }));
  });

  it("refuses deletion while a room turn is queued", () => {
    const room = createRoom(db, { name: "Planning", botIds: [alice.id] });
    const item = enqueue(db, { botId: alice.id, source: "room", text: "Work", conversationId: room.id });
    expect(() => deleteRoom(db, room.id)).toThrow(expect.objectContaining({ code: "busy" }));
    expect(getRoom(db, room.id)).toMatchObject({ name: "Planning" });
    db.prepare("UPDATE work_items SET status = 'running' WHERE id = ?").run(item.id);
    expect(() => deleteRoom(db, room.id)).toThrow(expect.objectContaining({ code: "busy" }));
    db.prepare("UPDATE work_items SET status = 'done' WHERE id = ?").run(item.id);
    deleteRoom(db, room.id);
  });

  it("cascades a deleted bot out of room membership", () => {
    const room = createRoom(db, { name: "Planning", botIds: [alice.id, bob.id] });
    deleteBot(db, alice.id);
    expect(getRoom(db, room.id).botIds).toEqual([bob.id]);
    expect(getRoom(db, TEAM_ROOM_ID).botIds).toEqual([bob.id]);
  });
});
