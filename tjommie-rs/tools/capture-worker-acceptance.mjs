// Regenerates tests/fixtures/compat/worker-acceptance.json: what the 0.4.2 TypeScript acceptance runner
// (scripts/vault/accept-worker.ts) wrote for a synthetic pair of worker identities, byte for byte, plus the two
// `node -e` programs it ran its probe and its peer holder with. The Rust tests pin the Rust daemon to these bytes, so a
// data root that 0.5.x used can still be rolled back to 0.4.2.
//
// It runs the legacy code, not a copy of it: it reads the legacy sources, lifts the exact lines it needs (the zod
// configuration schema and its hash, the Denials schema, the receipt literal, nativeNode and the two PowerShell
// programs) and evaluates them in a throwaway module with this repo's zod. Nothing touches Windows, Credential
// Manager or the network. The only output is the JSON, written to --out (else to stdout).
//
// Needs: Node 24 (it runs TypeScript), `npm ci` at the repo root (for zod), and a checkout of the `legacy` branch,
// which is where those sources live. A bare export of the two files is enough:
//   git archive origin/legacy packages/daemon/src/windows-worker.ts scripts/vault/accept-worker.ts | tar -x -C SOME_EMPTY_DIR
// Run it from the repo root:
//   node tjommie-rs/tools/capture-worker-acceptance.mjs --legacy SOME_EMPTY_DIR --legacy-commit "$(git rev-parse origin/legacy)" --out tjommie-rs/crates/daemon/tests/fixtures/compat/worker-acceptance.json
// Then check the bytes (for example with od -c) and run: cargo test -p daemon --test windows_worker_acceptance
//
// This file contains no backslash on purpose. Tool parameters and shells decode escapes, which once turned this
// fixture's Windows paths into "C:ProgramData..." and carriage returns. Windows paths are joined with
// String.fromCharCode(92), and the sources are split on character codes rather than escape sequences.
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, win32 } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const BACKSLASH = String.fromCharCode(92);
const BACKTICK = String.fromCharCode(96);
const NEWLINE = String.fromCharCode(10);
const CARRIAGE_RETURN = String.fromCharCode(13);

const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  return at >= 0 ? args[at + 1] : undefined;
};
const legacy = option("--legacy");
if (!legacy) {
  console.error("Usage: node capture-worker-acceptance.mjs --legacy <dir with the legacy sources> [--legacy-commit <sha>] [--out <file>]");
  process.exit(2);
}

const sourceLines = (file) => readFileSync(join(legacy, file), "utf8").split(CARRIAGE_RETURN).join("").split(NEWLINE);
const worker = sourceLines("packages/daemon/src/windows-worker.ts");
const runner = sourceLines("scripts/vault/accept-worker.ts");

// Lifts the lines from the first one that starts with `start` (ignoring indentation) up to the first later one that
// satisfies `ends`, or just that one line. A legacy source that no longer has them stops the run.
const lift = (lines, start, ends) => {
  const from = lines.findIndex((line) => line.trim().startsWith(start));
  if (from < 0) throw new Error("The legacy source has no line starting with: " + start);
  if (!ends) return lines[from];
  const to = lines.findIndex((line, index) => index > from && ends(line));
  if (to < 0) throw new Error("The legacy source has no end for the block starting: " + start);
  return lines.slice(from, to + 1).join(NEWLINE);
};

const lifted = {
  sid: lift(worker, "const Sid = "),
  schema: lift(worker, "export const WindowsWorkerConfiguration = z.object({", (line) => line.endsWith("}).strict();")),
  type: lift(worker, "export type WindowsWorkerConfiguration = "),
  hash: lift(worker, "export function workerConfigurationHash("),
  denials: lift(runner, "const Denials = z.object("),
  probe: lift(runner, "const powershellProbe = String.raw" + BACKTICK, (line) => line === BACKTICK + ";"),
  holder: lift(runner, "const powershellHolder = String.raw" + BACKTICK, (line) => line === BACKTICK + ";"),
  nativeNode: lift(runner, "const nativeNode = ", (line) => line.endsWith('");')),
  accepted: lift(runner, "const accepted = {"),
};

// ---- synthetic inputs: a pair that looks like what provision writes, in a root that is nobody's ----------------------
const windows = (...parts) => parts.join(BACKSLASH);
// The credential namespace of the owner root below: Namespace::new gives it (see tests/windows_worker.rs).
const namespace = "c7b7c5a986b63824c0803169";
const sid = (rid) => "S-1-5-21-1111111111-2222222222-3333333333-" + rid;
const ownerDataRoot = windows("C:", "Users", "Fixture", "AppData", "Local", "Tjommie-rs");
const base = windows("C:", "ProgramData", "Tjommie", "Worker-" + namespace);
const configuration = (purpose) => {
  const provider = purpose === "provider";
  return {
    version: 2,
    purpose,
    ownerSid: sid(1001),
    workerSid: sid(provider ? 1002 : 1003),
    peerSid: sid(provider ? 1003 : 1002),
    workerName: (provider ? "TjWorker-" : "TjReview-") + namespace.slice(0, 8),
    workerRoot: windows(base, provider ? "work" : "review"),
    peerRoot: windows(base, provider ? "review" : "work"),
    runtimeRoot: windows(base, "runtime"),
    ownerDataRoot,
    provisionedAt: "2026-11-09T12:00:00.000Z",
    credentialTarget: "Tjommie/" + namespace + "/isolated-" + (provider ? "worker" : "review"),
  };
};
const everyCheckHeld = {
  ownerFileDenied: true,
  ownerCredentialDenied: true,
  ownerProcessDenied: true,
  ownerDesktopDenied: true,
  workerWriteAllowed: true,
  peerFileDenied: true,
  peerWriteDenied: true,
  peerRegistryDenied: true,
  peerProcessDenied: true,
};
const inputs = { provider: configuration("provider"), review: configuration("review"), denials: everyCheckHeld };

// ---- the throwaway module: legacy lines verbatim, then the way accept-worker.ts used them --------------------------
const source = [
  'import { createHash } from "node:crypto";',
  'import { z } from "zod";',
  lifted.sid,
  lifted.schema,
  lifted.type,
  lifted.hash,
  lifted.denials,
  lifted.probe,
  lifted.holder,
  lifted.nativeNode,
  "const inputs = " + JSON.stringify(inputs) + ";",
  "const provider = WindowsWorkerConfiguration.parse(inputs.provider);",
  "const review = WindowsWorkerConfiguration.parse(inputs.review);",
  "const denials = Denials.parse(inputs.denials);",
  "const results = new Map([[" + JSON.stringify("provider") + ", denials], [" + JSON.stringify("review") + ", denials]]);",
  "const receipt = (boundary: string, config: WindowsWorkerConfiguration, peer: WindowsWorkerConfiguration): string => {",
  lifted.accepted,
  "  return JSON.stringify(accepted);",
  "};",
  "export default {",
  "  configurations: { provider, review },",
  "  denials,",
  "  configurationHashes: { provider: workerConfigurationHash(provider), review: workerConfigurationHash(review) },",
  '  receipts: { provider: receipt("provider", provider, review), review: receipt("review", review, provider) },',
  "  wrappers: { probe: nativeNode(powershellProbe, false), holder: nativeNode(powershellHolder, true) },",
  "};",
  "",
].join(NEWLINE);

const folder = mkdtempSync(join(dirname(fileURLToPath(import.meta.url)), ".capture-"));
let captured;
try {
  writeFileSync(join(folder, "module.ts"), source);
  captured = (await import(pathToFileURL(join(folder, "module.ts")).href)).default;
} finally {
  rmSync(folder, { recursive: true, force: true });
}

// A mangled path (a lost backslash, a control character) must not get into the fixture again.
for (const config of Object.values(captured.configurations)) {
  for (const key of ["workerRoot", "peerRoot", "runtimeRoot", "ownerDataRoot"]) {
    const value = config[key];
    if (!win32.isAbsolute(value) || !value.includes(BACKSLASH) || [...value].some((character) => character.charCodeAt(0) < 32)) {
      throw new Error(key + " is not a clean absolute Windows path");
    }
  }
}

const commit = option("--legacy-commit");
const out = {
  _provenance:
    "Generated by tjommie-rs/tools/capture-worker-acceptance.mjs, which lifts the zod WindowsWorkerConfiguration schema and workerConfigurationHash from packages/daemon/src/windows-worker.ts, and the Denials schema, the receipt literal, nativeNode and the two PowerShell programs from scripts/vault/accept-worker.ts, out of a checkout of the `legacy` branch" +
    (commit ? " (origin/legacy at " + commit + ")" : "") +
    ", and runs them on synthetic inputs; nothing touched Windows. `receipts` is the exact bytes accept-worker.ts wrote to worker-acceptance.json (provider) and review-worker-acceptance.json (review). `wrappers` are the two `node -e` programs it ran the denial probe and the peer holder with: nativeNode(powershellProbe, false) and nativeNode(powershellHolder, true). Do not change this file unless rollback compatibility with 0.4.2 is deliberately dropped.",
  ...captured,
};
const json = JSON.stringify(out, null, 2) + NEWLINE;
const target = option("--out");
if (target) writeFileSync(target, json);
else process.stdout.write(json);
