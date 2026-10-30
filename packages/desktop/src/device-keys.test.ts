import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import type { CredentialTransport } from "../../daemon/src/account-credentials.ts";
import { createDeviceKeys } from "./device-keys.ts";

it("seals, reads and deletes a device key through a synthetic credential transport", async () => {
  const records = new Map<string, string>();
  const operations: string[] = [];
  const transport: CredentialTransport = async ({ operation, target, key }) => {
    operations.push(`${operation}:${target}`);
    if (operation === "read") return records.get(target) ?? null;
    if (operation === "write" && key !== undefined) records.set(target, key);
    if (operation === "delete") records.delete(target);
    return null;
  };
  const dataRoot = "C:/isolated/tjommie-test";
  const keys = createDeviceKeys(dataRoot, transport);
  const name = "identity-5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c11.pending";
  const namespace = createHash("sha256").update(resolve(dataRoot).toLowerCase()).digest("hex").slice(0, 24);
  const target = `Tjommie/${namespace}/device/${name}`;
  expect(await keys.read(name)).toBeNull();
  const value = new Uint8Array([0, 1, 127, 255]);
  await keys.seal(name, value);
  expect(records.get(target)).toBe("AAF__w");
  expect(await keys.read(name)).toEqual(value);
  await keys.delete(name);
  expect(await keys.read(name)).toBeNull();
  expect(operations).toEqual([`read:${target}`, `write:${target}`, `read:${target}`, `delete:${target}`, `read:${target}`]);
});

it("separates device keys for two data roots, including deletion", async () => {
  const records = new Map<string, string>();
  const transport: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "read") return records.get(target) ?? null;
    if (operation === "write" && key !== undefined) records.set(target, key);
    if (operation === "delete") records.delete(target);
    return null;
  };
  const first = createDeviceKeys("C:/isolated/first", transport);
  const second = createDeviceKeys("C:/isolated/second", transport);
  await first.seal("identity", new Uint8Array([1]));
  await second.seal("identity", new Uint8Array([2]));
  expect(await first.read("identity")).toEqual(new Uint8Array([1]));
  expect(await second.read("identity")).toEqual(new Uint8Array([2]));
  await first.delete("identity");
  expect(await first.read("identity")).toBeNull();
  expect(await second.read("identity")).toEqual(new Uint8Array([2]));
  expect(records.size).toBe(1);
});

it("rejects unsafe names and invalid values without exposing them in errors", async () => {
  const transport: CredentialTransport = async () => { throw new Error("secret from transport"); };
  const keys = createDeviceKeys("C:/isolated/errors", transport);
  await expect(keys.seal("../escape", new Uint8Array([1]))).rejects.toThrow("Invalid device key.");
  await expect(keys.seal("valid", new Uint8Array())).rejects.toThrow("Invalid device key.");
  await expect(keys.read("valid")).rejects.toThrow(/^Device key storage is unavailable\.$/);
});
