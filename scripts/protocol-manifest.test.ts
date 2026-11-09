import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { events, methods } from "../packages/protocol/src/index.ts";
import { protocolManifest } from "./protocol-manifest.ts";

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

it("--check fails on a stale manifest and passes on a fresh one", () => {
  const dir = mkdtempSync(join(tmpdir(), "protocol-manifest-"));
  try {
    const file = join(dir, "manifest.json");
    writeFileSync(file, "{}\n");
    const stale = run(["--check", file]);
    expect(stale.status).toBe(1);
    expect(stale.stderr).toContain("npm run protocol:manifest");
    expect(run([file]).status).toBe(0);
    expect(run(["--check", file]).status).toBe(0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
