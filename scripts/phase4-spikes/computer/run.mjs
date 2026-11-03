import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const executable = require("electron");
const temporaryBase = resolve(tmpdir());
const root = mkdtempSync(join(temporaryBase, "tjommie-p4-computer-"));
assert.equal(dirname(resolve(root)), temporaryBase);
assert(basename(root).startsWith("tjommie-p4-computer-"));
const env = { ...process.env, TJOMMIE_PHASE4_COMPUTER_ROOT: root };
delete env.ELECTRON_RUN_AS_NODE;
try {
  const child = spawn(executable, [join(import.meta.dirname, "probe.ts")], { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  for (const stream of [child.stdout, child.stderr]) stream.on("data", (chunk) => { output = (output + chunk.toString()).slice(-32_768); });
  const timer = setTimeout(() => child.kill(), 60_000);
  let code;
  try {
    code = await new Promise((done, reject) => { child.once("error", reject); child.once("exit", done); });
  } finally { clearTimeout(timer); }
  process.stdout.write(output);
  assert.equal(code, 0, "Disposable Electron probe failed.");
  assert(output.includes("PHASE4_COMPUTER_RESULT"));
} finally {
  assert.equal(dirname(resolve(root)), temporaryBase);
  assert(basename(root).startsWith("tjommie-p4-computer-"));
  rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
}
