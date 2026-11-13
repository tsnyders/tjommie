import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { credentialNamespace, type CredentialTransport } from "./credential-transport.ts";
import { createDeviceKeys } from "./device-keys.ts";
import { WindowsLoginCredentials } from "./vault-storage.ts";

// The targets the 0.4.2 TypeScript modules asked Credential Manager for, captured once and read by the Rust daemon's own
// tests (cred_roundtrip.rs) too. If the desktop and the Rust daemon ever disagree on a namespace, one of these two fails,
// and an upgrade would stop finding the secrets 0.4.2 saved.
const captured = JSON.parse(readFileSync(resolve(import.meta.dirname, "..", "..", "..", "tjommie-rs", "crates", "daemon", "tests", "fixtures", "compat", "credential-entries.json"), "utf8")) as {
  root: string;
  login: { reference: string; secret: { username: string; password: string }; target: string; blobUtf8: string };
  remoteHostTarget: string;
  relayLoginTarget: string;
};

it("derives the namespace the Rust daemon derives: sha256 of the lowercased, resolved root, 24 hex (32 for the host keys)", () => {
  expect(credentialNamespace(captured.root)).toBe("c7b7c5a986b63824c0803169");
  expect(`Tjommie/${credentialNamespace(captured.root, 32)}/remote-host`).toBe(captured.remoteHostTarget);
  expect(`Tjommie/${credentialNamespace(captured.root, 32)}/relay-login`).toBe(captured.relayLoginTarget);
  // Another spelling of the same root lands on the same entries; another root does not.
  expect(credentialNamespace("c:/users/fixture/appdata/local/TJOMMIE-RS/")).toBe(credentialNamespace(captured.root));
  expect(credentialNamespace(`${captured.root}2`)).not.toBe(credentialNamespace(captured.root));
});

it("saves a Logins secret under the target and blob 0.4.2 used, so an upgraded app reads what was saved", async () => {
  const written: { operation: string; target: string; key?: string }[] = [];
  const transport: CredentialTransport = async (input) => { written.push(input); return null; };
  await new WindowsLoginCredentials(captured.root, transport).write(captured.login.reference, captured.login.secret);
  expect(written).toEqual([{ operation: "write", target: captured.login.target, key: captured.login.blobUtf8 }]);
});

it("saves a device key under its owner's namespace", async () => {
  const written: { operation: string; target: string }[] = [];
  const transport: CredentialTransport = async (input) => { written.push(input); return null; };
  await createDeviceKeys(captured.root, transport).seal("identity-host", new Uint8Array([1, 2, 3]));
  expect(written.map((entry) => entry.target)).toEqual(["Tjommie/c7b7c5a986b63824c0803169/device/identity-host"]);
});
