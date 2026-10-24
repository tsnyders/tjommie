import { afterEach, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { loadRemoteIdentity } from "./remote-identity.ts";
import type { CredentialTransport } from "./account-credentials.ts";

const db = openDb(":memory:");
afterEach(() => db.exec("DELETE FROM remote_host; DELETE FROM devices;"));

it("keeps the private identity in the injected vault and preserves it across loads", async () => {
  const values = new Map<string, string>();
  const vault: CredentialTransport = async (input) => {
    if (input.operation === "write" && input.key) values.set(input.target, input.key);
    return input.operation === "read" ? values.get(input.target) ?? null : null;
  };
  let generated = 0;
  const options = { db, root: "synthetic-root", name: "Synthetic PC", credentials: vault, generate: () => { generated++; return { privateKey: new Uint8Array(32).fill(7), publicKey: new Uint8Array(32).fill(8) }; } };
  const first = await loadRemoteIdentity(options);
  const second = await loadRemoteIdentity(options);
  expect(second).toEqual(first);
  expect(generated).toBe(1);
  expect(JSON.stringify(db.prepare("SELECT * FROM remote_host").all())).not.toContain(Buffer.from(first.privateKey).toString("base64url"));
  expect([...values.keys()][0]).toMatch(/^Tjommie\/[a-f0-9]{32}\/remote-host$/);
  await expect(loadRemoteIdentity({ ...options, credentials: async () => null })).rejects.toThrow("replacement identity");
});

it("does not write database identity when the vault write fails", async () => {
  const secret = new Uint8Array(32).fill(9);
  await expect(loadRemoteIdentity({ db, root: "synthetic-root", name: "PC", generate: () => ({ privateKey: secret, publicKey: new Uint8Array(32).fill(8) }), credentials: async (input) => {
    if (input.operation === "write") throw new Error("secret-must-not-escape");
    return null;
  } })).rejects.toThrow("Windows Credential Manager");
  expect(secret.every((value) => value === 0)).toBe(true);
  expect(db.prepare("SELECT * FROM remote_host").all()).toEqual([]);
});
