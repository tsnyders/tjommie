import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { events, methods } from "../packages/protocol/src/index.ts";
import { manifestText, protocolManifest } from "./protocol-manifest.ts";

const script = fileURLToPath(new URL("./protocol-manifest.ts", import.meta.url));
const run = (args: string[]) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8", windowsHide: true });

it("names every protocol method and event, in order", () => {
  const manifest = protocolManifest();
  expect(manifest.methods.map((entry) => entry.name)).toEqual(Object.keys(methods));
  expect(manifest.events.map((entry) => entry.name)).toEqual(Object.keys(events));
  expect(manifest.methods).toHaveLength(101);
  expect(manifest.events).toHaveLength(26);
});

it("describes params as the wire sends them, before defaults fill in", () => {
  const send = protocolManifest().methods.find((entry) => entry.name === "chat.send");
  expect(send?.params).toMatchObject({ type: "object", required: ["botId", "text"] });
});

it("marks strings zod trims before it checks their length", () => {
  const create = protocolManifest().methods.find((entry) => entry.name === "bots.create");
  expect(create?.params).toMatchObject({ properties: { name: { "x-trim": true, minLength: 1 }, model: { minLength: 1 } } });
  expect(create?.params).not.toMatchObject({ properties: { model: { "x-trim": true } } });
  // zod's URL check trims too, before .max applies.
  const settings = protocolManifest().methods.find((entry) => entry.name === "settings.set");
  expect(JSON.stringify(settings?.params)).toContain('"x-trim":true');
});

it("exports the standalone schemas parsed outside method calls", () => {
  const names = protocolManifest().types.map((entry) => entry.name);
  for (const name of ["ClientMessage", "MessageData", "PairingCode", "RelayToHost", "NativeBrowserCommand", "VaultHostJob", "DiagnosticReport"]) expect(names).toContain(name);
  expect(protocolManifest().types.every((entry) => entry.schema !== "custom")).toBe(true);
});

// Each check runs the script in a fresh node process, which exports every schema again: slow on a busy machine.
const check = (text: string) => {
  const dir = mkdtempSync(join(tmpdir(), "protocol-manifest-"));
  try {
    const file = join(dir, "manifest.json");
    writeFileSync(file, text);
    return run(["--check", file]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
};

it("--check fails on a stale manifest", () => {
  const stale = check("{}\n");
  expect(stale.status).toBe(1);
  expect(stale.stderr).toContain("npm run protocol:manifest");
}, 60_000);

it("--check passes on a fresh manifest, whatever its line endings", () => {
  expect(check(manifestText().replaceAll("\n", "\r\n")).status).toBe(0);
}, 60_000);
