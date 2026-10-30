import { createPublicKey, generateKeyPairSync, verify } from "node:crypto";
import { expect, it } from "vitest";
import type { CredentialTransport } from "./account-credentials.ts";
import { openDb } from "./db.ts";
import { loadRelayIdentity } from "./relay-identity.ts";
import { loadRemoteIdentity } from "./remote-identity.ts";

it("round-trips an Ed25519 login key through a synthetic credential vault", async () => {
  const values = new Map<string, string>();
  let writes = 0;
  const credentials: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "write" && key) { writes++; values.set(target, key); }
    return operation === "read" ? values.get(target) ?? null : null;
  };
  const root = "synthetic-relay-root";
  const first = await loadRelayIdentity({ root, credentials });
  const second = await loadRelayIdentity({ root, credentials });
  const message = new TextEncoder().encode("synthetic login challenge");
  const publicKey = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(first.publicKey).toString("base64url") }, format: "jwk" });
  expect(second.publicKey).toEqual(first.publicKey);
  expect(verify(null, message, publicKey, first.sign(message))).toBe(true);
  expect(verify(null, message, publicKey, second.sign(message))).toBe(true);
  expect(writes).toBe(1);
  const db = openDb(":memory:");
  try {
    await loadRemoteIdentity({ db, root, name: "Synthetic PC", credentials,
      generate: () => ({ privateKey: new Uint8Array(32).fill(3), publicKey: new Uint8Array(32).fill(4) }) });
  } finally { db.close(); }
  const targets = [...values.keys()];
  expect(targets[0]).toMatch(/^Tjommie\/[a-f0-9]{32}\/relay-login$/);
  expect(targets[1]).toBe(targets[0].replace(/relay-login$/, "remote-host"));
});

it("rejects a mismatched saved key without minting a replacement", async () => {
  let saved: string | null = null;
  let writes = 0;
  const credentials: CredentialTransport = async ({ operation, key }) => {
    if (operation === "write" && key) { saved = key; writes++; }
    return operation === "read" ? saved : null;
  };
  await loadRelayIdentity({ root: "synthetic-relay-root", credentials });
  const record = JSON.parse(saved!) as { v: number; privateKey: string; publicKey: string };
  saved = JSON.stringify({ ...record, publicKey: Buffer.alloc(32, 9).toString("base64url") });
  await expect(loadRelayIdentity({ root: "synthetic-relay-root", credentials })).rejects.toThrow("connection key");
  expect(writes).toBe(1);
});

it("keeps corrupt and non-Ed25519 vault contents out of errors without minting", async () => {
  let saved = '{"privateKey":"synthetic-sensitive-key",';
  let writes = 0;
  const credentials: CredentialTransport = async ({ operation }) => { if (operation === "write") writes++; return operation === "read" ? saved : null; };
  const options = { root: "synthetic-relay-root", credentials };
  await expect(loadRelayIdentity(options)).rejects.toThrow("connection key");
  try { await loadRelayIdentity(options); } catch (error) { expect(String(error)).not.toContain("synthetic-sensitive-key"); }
  const pair = generateKeyPairSync("x25519");
  saved = JSON.stringify({ v: 1, privateKey: pair.privateKey.export({ format: "der", type: "pkcs8" }).toString("base64url"),
    publicKey: pair.publicKey.export({ format: "der", type: "spki" }).subarray(-32).toString("base64url") });
  await expect(loadRelayIdentity(options)).rejects.toThrow("connection key");
  expect(writes).toBe(0);
});
