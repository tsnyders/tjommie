import { TEAM_ROOM_ID, type Room, type RoomInput, type RoomPatch } from "@tjommie/protocol";
import { getStoredBot, listStoredBots, type StoredBot } from "./bots.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

type RoomRow = { id: string; name: string; created_at: string | null };

export function listRooms(db: Db): Room[] {
  const rows = db.prepare("SELECT id FROM conversations WHERE kind = 'room' ORDER BY CASE WHEN id = ? THEN 0 ELSE 1 END, created_at, rowid").all(TEAM_ROOM_ID) as { id: string }[];
  return rows.map((row) => getRoom(db, row.id));
}

export function getRoom(db: Db, id: string): Room {
  const row = db.prepare("SELECT id, name, created_at FROM conversations WHERE id = ? AND kind = 'room'").get(id) as RoomRow | undefined;
  if (!row) throw new RpcError("not_found", "No such room");
  return { id, name: row.name, botIds: roomMembers(db, id).map((bot) => bot.id), everyone: id === TEAM_ROOM_ID, createdAt: row.created_at };
}

export function roomMembers(db: Db, roomId: string): StoredBot[] {
  if (roomId === TEAM_ROOM_ID) return listStoredBots(db);
  const rows = db.prepare("SELECT b.id FROM bots b JOIN room_members m ON m.bot_id = b.id WHERE m.room_id = ? ORDER BY b.created_at, b.rowid").all(roomId) as { id: string }[];
  return rows.map((row) => getStoredBot(db, row.id));
}

export function createRoom(db: Db, input: RoomInput): Room {
  assertNameFree(db, input.name, null);
  const id = newId();
  transaction(db, () => {
    db.prepare("INSERT INTO conversations (id, kind, bot_id, name, created_at) VALUES (?, 'room', NULL, ?, ?)").run(id, input.name, nowIso());
    setMembers(db, id, input.botIds);
  });
  return getRoom(db, id);
}

export function updateRoom(db: Db, id: string, patch: RoomPatch): Room {
  editable(db, id);
  if (patch.name !== undefined) assertNameFree(db, patch.name, id);
  transaction(db, () => {
    if (patch.name !== undefined) db.prepare("UPDATE conversations SET name = ? WHERE id = ?").run(patch.name, id);
    if (patch.botIds !== undefined) setMembers(db, id, patch.botIds);
  });
  return getRoom(db, id);
}

export function deleteRoom(db: Db, id: string): void {
  editable(db, id);
  if (db.prepare("SELECT 1 FROM work_items WHERE conversation_id = ? AND status IN ('queued', 'running') LIMIT 1").get(id)) {
    throw new RpcError("busy", "This room has a bot still answering");
  }
  db.prepare("DELETE FROM conversations WHERE id = ?").run(id);
}

function editable(db: Db, id: string): void {
  if (id === TEAM_ROOM_ID) throw new RpcError("bad_state", "Everyone cannot be changed");
  getRoom(db, id);
}

function assertNameFree(db: Db, name: string, exceptId: string | null): void {
  if (db.prepare("SELECT id FROM conversations WHERE kind = 'room' AND lower(name) = lower(?) AND id IS NOT ?").get(name, exceptId)) {
    throw new RpcError("name_taken", `There's already a room called ${name}`);
  }
}

function setMembers(db: Db, roomId: string, botIds: string[]): void {
  const unique = [...new Set(botIds)];
  for (const botId of unique) {
    if (!db.prepare("SELECT 1 FROM bots WHERE id = ?").get(botId)) throw new RpcError("bad_params", "Room member: no such bot");
  }
  db.prepare("DELETE FROM room_members WHERE room_id = ?").run(roomId);
  for (const botId of unique) db.prepare("INSERT INTO room_members (room_id, bot_id) VALUES (?, ?)").run(roomId, botId);
}
