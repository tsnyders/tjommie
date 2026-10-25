import { afterEach, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { addDevice, clearPushToken, getDevice, listDevices, pushTargets, renameDevice, revokeDevice, setPushToken, touchDevice } from "./devices-store.ts";

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
  expect(pushTargets(db)).toEqual([{ deviceId: device.id, token: "synthetic-token" }]);
  clearPushToken(db, device.id);
  expect(pushTargets(db)).toEqual([]);
  setPushToken(db, device.id, "synthetic-token");
  revokeDevice(db, device.id);
  expect(pushTargets(db)).toEqual([]);
  expect(db.prepare("SELECT push_token FROM devices WHERE id = ?").get(device.id)).toEqual({ push_token: null });
  db.prepare("UPDATE devices SET push_token = ? WHERE id = ?").run("stale-token", device.id);
  expect(pushTargets(db)).toEqual([]);
  expect(() => setPushToken(db, device.id, "new-token")).toThrow();
});

it("moves a token to the latest active device registration", () => {
  const first = addDevice(db, { publicKey: key, name: "Old pairing" });
  const second = addDevice(db, { publicKey: Buffer.alloc(32, 2).toString("base64url"), name: "New pairing" });
  setPushToken(db, first.id, "shared-install-token");
  setPushToken(db, second.id, "shared-install-token");
  expect(db.prepare("SELECT push_token FROM devices WHERE id = ?").get(first.id)).toEqual({ push_token: null });
  expect(pushTargets(db)).toEqual([{ deviceId: second.id, token: "shared-install-token" }]);
});
