import { afterEach, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { addDevice, getDevice, listDevices, renameDevice, revokeDevice, touchDevice } from "./devices-store.ts";

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
