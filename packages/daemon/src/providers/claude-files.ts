import { lstatSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

type SessionState = { phase: "preparing" | "starting"; childPid: null } | { phase: "started"; childPid: number };
type SessionOwner = SessionState & { version: 1; ownerPid: number };

export function recordClaudeSessionOwner(folder: string, state: SessionState): void {
  const file = join(folder, "owner.json");
  const temporary = `${file}.tmp`;
  writeFileSync(temporary, JSON.stringify({ version: 1, ownerPid: process.pid, ...state }), { mode: 0o600 });
  renameSync(temporary, file);
}

// Account folders are shared by daemon processes and real tests. A live owner or CLI,
// an unknown process state, or an incomplete launch marker must keep its files.
export function sweepClaudeSessionFiles(accountFolder: string): void {
  try {
    for (const entry of readdirSync(accountFolder, { withFileTypes: true })) {
      if (!entry.name.startsWith(".tjommie-session-") || !entry.isDirectory() || entry.isSymbolicLink()) continue;
      const folder = join(accountFolder, entry.name);
      try {
        const owner = readOwner(folder);
        if (!owner || owner.phase === "starting" || !processIsGone(owner.ownerPid)) continue;
        if (owner.phase === "started" && !processIsGone(owner.childPid)) continue;
        const stat = lstatSync(folder);
        if (stat.isDirectory() && !stat.isSymbolicLink()) rmSync(folder, { recursive: true, force: true });
      } catch (error) {
        if (errorCode(error) !== "ENOENT") console.error("Could not remove stale Claude session files.", folder);
      }
    }
  } catch (error) {
    if (errorCode(error) !== "ENOENT") console.error("Could not check Claude's session files.", accountFolder);
  }
}

function readOwner(folder: string): SessionOwner | null {
  const file = join(folder, "owner.json");
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024) return null;
  let owner: unknown;
  try {
    owner = JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    if (error instanceof SyntaxError) return null;
    throw error;
  }
  if (!isRecord(owner) || owner.version !== 1 || !isPid(owner.ownerPid)) return null;
  if ((owner.phase === "preparing" || owner.phase === "starting") && owner.childPid === null) {
    return { version: 1, ownerPid: owner.ownerPid, phase: owner.phase, childPid: null };
  }
  if (owner.phase === "started" && isPid(owner.childPid)) {
    return { version: 1, ownerPid: owner.ownerPid, phase: "started", childPid: owner.childPid };
  }
  return null;
}

function processIsGone(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return false;
  } catch (error) {
    return errorCode(error) === "ESRCH";
  }
}

function isPid(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function errorCode(error: unknown): unknown {
  return isRecord(error) ? error.code : undefined;
}
