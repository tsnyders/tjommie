import { Device, RemoteKey, RemoteName } from "../../protocol/src/remote.ts";
import { z } from "zod";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

const columns = "id, name, public_key AS publicKey, paired_at AS pairedAt, last_seen_at AS lastSeenAt, revoked_at AS revokedAt";
const PushTarget = z.object({ deviceId: z.uuid(), token: z.string().min(1).max(4096) });
export function listDevices(db: Db): Device[] {
  return db.prepare(`SELECT ${columns} FROM devices ORDER BY paired_at, id`).all().map((row) => Device.parse(row));
}
export function getDevice(db: Db, id: string): Device {
  const row = db.prepare(`SELECT ${columns} FROM devices WHERE id = ?`).get(id);
  if (!row) throw new RpcError("not_found", "This device was not found.");
  return Device.parse(row);
}
export function deviceForKey(db: Db, key: string): Device | null {
  // Include revoked keys so the host can send an authenticated refusal.
  const row = db.prepare(`SELECT ${columns} FROM devices WHERE public_key = ?`).get(key);
  return row ? Device.parse(row) : null;
}
export function pushTargets(db: Db): { deviceId: string; token: string }[] {
  return db.prepare("SELECT id AS deviceId, push_token AS token FROM devices WHERE revoked_at IS NULL AND push_token IS NOT NULL").all().map((row) => PushTarget.parse(row));
}
export function setPushToken(db: Db, id: string, token: string | null): void {
  transaction(db, () => {
    if (getDevice(db, id).revokedAt) throw new RpcError("bad_state", "This device has been revoked.");
    if (token !== null) db.prepare("UPDATE devices SET push_token = NULL WHERE push_token = ? AND id <> ?").run(token, id);
    db.prepare("UPDATE devices SET push_token = ? WHERE id = ? AND revoked_at IS NULL").run(token, id);
  });
}
export function clearPushToken(db: Db, id: string): void {
  db.prepare("UPDATE devices SET push_token = NULL WHERE id = ?").run(id);
}
export function addDevice(db: Db, input: { publicKey: string; name: string }): Device {
  const publicKey = RemoteKey.parse(input.publicKey), name = RemoteName.parse(input.name);
  if (db.prepare("SELECT id FROM devices WHERE public_key = ?").get(publicKey)) throw new RpcError("bad_state", "This device key was already paired. Generate a new device identity before pairing again.");
  const total = db.prepare("SELECT count(*) AS total FROM devices").get()?.total;
  if (typeof total !== "number" || total >= 64) throw new RpcError("busy", "The device list is full.");
  const id = newId();
  db.prepare("INSERT INTO devices(id,name,public_key,paired_at) VALUES(?,?,?,?)").run(id, name, publicKey, nowIso());
  return getDevice(db, id);
}
export function renameDevice(db: Db, id: string, value: string): Device {
  const name = RemoteName.parse(value);
  getDevice(db, id);
  db.prepare("UPDATE devices SET name = ? WHERE id = ?").run(name, id);
  return getDevice(db, id);
}
export function revokeDevice(db: Db, id: string): Device {
  getDevice(db, id);
  db.prepare("UPDATE devices SET revoked_at = COALESCE(revoked_at, ?), push_token = NULL WHERE id = ?").run(nowIso(), id);
  return getDevice(db, id);
}
export function touchDevice(db: Db, id: string, at = nowIso()): void {
  if (getDevice(db, id).revokedAt) throw new RpcError("bad_state", "This device has been revoked.");
  db.prepare("UPDATE devices SET last_seen_at = ? WHERE id = ? AND revoked_at IS NULL").run(at, id);
}
