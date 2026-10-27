import { afterEach, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { addDevice, clearPushToken, getDevice, listDevices, pushTargets, renameDevice, revokeDevice, setPushToken, touchDevice } from "./devices-store.ts";
import { startPush } from "./push.ts";

const db = openDb(":memory:");
afterEach(() => db.exec("DELETE FROM devices"));
const key = "AQ".padEnd(43, "A");

it("persists paired device identity, rename and last seen while revocation is irreversible", () => {
  const device = addDevice(db, { publicKey: key, name: "My phone" });
  expect(getDevice(db, device.id)).toEqual(device);
  expect(listDevices(db)).toHaveLength(1);
  renameDevice(db, device.id, "Phone renamed");
  touchDevice(db, device.id, "2026-10-22T12:00:00.000Z");
  expect(getDevice(db, device.id)).toMatchObject({ name: "Phone renamed", lastSeenAt: "2026-10-22T12:00:00.000Z", revokedAt: null });
  revokeDevice(db, device.id);
  expect(getDevice(db, device.id).revokedAt).not.toBeNull();
  expect(() => addDevice(db, { publicKey: key, name: "Same key" })).toThrow();
  expect(() => touchDevice(db, device.id)).toThrow();
});

it("rejects invalid names and keys before writing", () => {
  expect(() => addDevice(db, { publicKey: "bad", name: "Phone" })).toThrow();
  expect(() => addDevice(db, { publicKey: key, name: "\u0000" })).toThrow();
  expect(listDevices(db)).toEqual([]);
});

it("stores push tokens separately from device details and clears them on revoke", () => {
  const device = addDevice(db, { publicKey: key, name: "Phone" });
  setPushToken(db, device.id, "synthetic-token");
  expect(getDevice(db, device.id)).toEqual(device);
  expect(pushTargets(db)).toEqual([{ deviceId: device.id, token: "synthetic-token", kinds: null }]);
  clearPushToken(db, device.id, "synthetic-token");
  expect(pushTargets(db)).toEqual([]);
  setPushToken(db, device.id, "synthetic-token");
  revokeDevice(db, device.id);
  expect(pushTargets(db)).toEqual([]);
  expect(db.prepare("SELECT push_token, push_kinds FROM devices WHERE id = ?").get(device.id)).toEqual({ push_token: null, push_kinds: null });
  db.prepare("UPDATE devices SET push_token = ? WHERE id = ?").run("stale-token", device.id);
  expect(pushTargets(db)).toEqual([]);
  expect(() => setPushToken(db, device.id, "new-token")).toThrow();
});

it("keeps a newly registered token when an older push is reported unregistered", () => {
  const device = addDevice(db, { publicKey: key, name: "Phone" });
  setPushToken(db, device.id, "old-token");
  setPushToken(db, device.id, "new-token");
  clearPushToken(db, device.id, "old-token");
  expect(pushTargets(db)).toEqual([{ deviceId: device.id, token: "new-token", kinds: null }]);
  clearPushToken(db, device.id, "new-token");
  expect(pushTargets(db)).toEqual([]);
});

it("moves a token to the latest active device registration", () => {
  const first = addDevice(db, { publicKey: key, name: "Old pairing" });
  const second = addDevice(db, { publicKey: Buffer.alloc(32, 2).toString("base64url"), name: "New pairing" });
  setPushToken(db, first.id, "shared-install-token");
  setPushToken(db, second.id, "shared-install-token");
  expect(db.prepare("SELECT push_token FROM devices WHERE id = ?").get(first.id)).toEqual({ push_token: null });
  expect(pushTargets(db)).toEqual([{ deviceId: second.id, token: "shared-install-token", kinds: null }]);
});

it("stores explicit push kinds per device and clears them on revoke", () => {
  const device = addDevice(db, { publicKey: key, name: "Phone" });
  setPushToken(db, device.id, "synthetic-token", ["done"]);
  expect(pushTargets(db)).toEqual([{ deviceId: device.id, token: "synthetic-token", kinds: ["done"] }]);
  expect(db.prepare("SELECT push_kinds FROM devices WHERE id = ?").get(device.id)).toEqual({ push_kinds: '["done"]' });
  setPushToken(db, device.id, "synthetic-token", []);
  expect(pushTargets(db)[0].kinds).toEqual([]);
  revokeDevice(db, device.id);
  expect(db.prepare("SELECT push_token, push_kinds FROM devices WHERE id = ?").get(device.id)).toEqual({ push_token: null, push_kinds: null });
});

it("keeps every push target available when stored kinds are malformed or newer", () => {
  const malformed = addDevice(db, { publicKey: key, name: "Malformed" });
  const unknown = addDevice(db, { publicKey: Buffer.alloc(32, 2).toString("base64url"), name: "Unknown" });
  const mixed = addDevice(db, { publicKey: Buffer.alloc(32, 3).toString("base64url"), name: "Mixed" });
  const valid = addDevice(db, { publicKey: Buffer.alloc(32, 4).toString("base64url"), name: "Valid" });
  for (const device of [malformed, unknown, mixed, valid]) setPushToken(db, device.id, `token-${device.name}`);
  db.prepare("UPDATE devices SET push_kinds = ? WHERE id = ?").run("not json", malformed.id);
  db.prepare("UPDATE devices SET push_kinds = ? WHERE id = ?").run('["mention"]', unknown.id);
  db.prepare("UPDATE devices SET push_kinds = ? WHERE id = ?").run('["done","mention"]', mixed.id);
  const targets = pushTargets(db);
  expect(targets).toHaveLength(4);
  expect(targets).toContainEqual({ deviceId: malformed.id, token: "token-Malformed", kinds: null });
  expect(targets).toContainEqual({ deviceId: unknown.id, token: "token-Unknown", kinds: [] });
  expect(targets).toContainEqual({ deviceId: mixed.id, token: "token-Mixed", kinds: ["done"] });
  const sent: string[][] = [];
  const push = startPush({ host: { pushTargets: () => pushTargets(db), clearPushToken: () => {} },
    relay: { kinds: () => ["done"] as const, push: async (tokens) => { sent.push(tokens); return tokens.map(() => ({ ok: true, unregistered: false })); } } });
  push({ kind: "done", botId: "bot", title: "Done", body: "" });
  expect(sent.flat().sort()).toEqual(["token-Malformed", "token-Mixed", "token-Valid"].sort());
});
