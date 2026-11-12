import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import type { BotInput } from "@tjommie/protocol";
import { createBot } from "../packages/daemon/src/bots.ts";
import { addMessage } from "../packages/daemon/src/chat.ts";
import { openDb } from "../packages/daemon/src/db.ts";
import { ensureAccounts } from "../packages/daemon/src/setup.ts";
import { recordUsage } from "../packages/daemon/src/usage.ts";
import { packagedDaemonPath, startDaemonProcess, type DaemonLaunch } from "../packages/desktop/src/daemon-process.ts";

// The R1 acceptance run (.github/workflows/r1-acceptance.yml): the Rust daemon takes over a data root that a 0.4.2 install
// seeded, and 0.4.2 must still open it afterwards (spec §5, release R1).
//
//   seed    --root <dir> --counts <file>     a 0.4.2 data root, and its row counts
//   install --installer <file>               installs silently, prints the install folder
//   start   --install <dir> --root <dir>     runs the installed app's own daemon on the root, then stops it
//   verify  --root <dir> --counts <file> --stage upgraded|rolledback [--version <v>]
//   files   --dir <dir> [--installed] [--same-as <dir>] [--equals <n>]    prints the file count
//
// Everything but `files` changes the machine, so it refuses to run anywhere but a GitHub Actions runner. This is never run
// against a real Tjommie data root or install.

export type Counts = { userVersion: number; tables: Record<string, number>; botFiles: number };

export function assertCi(env: NodeJS.ProcessEnv = process.env): void {
  if (env.GITHUB_ACTIONS !== "true") throw new Error("The R1 acceptance run changes a data root and an install, so it runs only on a GitHub Actions runner.");
}

// ---- Counting ----

export function listFiles(dir: string, installed = false): string[] {
  const files: string[] = [];
  const visit = (folder: string): void => {
    for (const entry of readdirSync(folder, { withFileTypes: true })) {
      const path = join(folder, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile()) files.push(relative(dir, path).replaceAll("\\", "/").toLowerCase());
    }
  };
  visit(dir);
  // NSIS adds its uninstaller to an install; a fresh build's unpacked folder has none.
  return files.filter((file) => !(installed && /^uninstall [^/]*\.exe$/.test(file))).sort();
}

export function diffFiles(expected: string[], found: string[]): string[] {
  const has = new Set(found);
  const had = new Set(expected);
  return [...expected.filter((file) => !has.has(file)).map((file) => `missing: ${file}`), ...found.filter((file) => !had.has(file)).map((file) => `extra: ${file}`)];
}

const quoted = (name: string): string => `"${name.replaceAll('"', '""')}"`;

export function readCounts(dbFile: string, botsDir?: string): Counts {
  const db = new DatabaseSync(dbFile);
  try {
    const { user_version } = db.prepare("PRAGMA user_version").get() as { user_version: number };
    const names = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all() as { name: string }[];
    const tables: Record<string, number> = {};
    for (const { name } of names) tables[name] = (db.prepare(`SELECT COUNT(*) AS n FROM ${quoted(name)}`).get() as { n: number }).n;
    return { userVersion: user_version, tables, botFiles: botsDir && existsSync(botsDir) ? listFiles(botsDir).length : 0 };
  } finally {
    db.close();
  }
}

function integrity(dbFile: string): string {
  const db = new DatabaseSync(dbFile);
  try {
    return (db.prepare("PRAGMA integrity_check").all() as { integrity_check: string }[]).map((row) => row.integrity_check).join("; ");
  } finally {
    db.close();
  }
}

export function diffCounts(label: string, expected: Counts, found: Counts): string[] {
  const problems: string[] = [];
  if (found.userVersion !== expected.userVersion) problems.push(`${label}: user_version is ${found.userVersion}, expected ${expected.userVersion}`);
  for (const name of new Set([...Object.keys(expected.tables), ...Object.keys(found.tables)])) {
    const want = expected.tables[name];
    const have = found.tables[name];
    if (want !== have) problems.push(`${label}: table ${name} has ${have ?? "no such table"} rows, expected ${want ?? "no such table"}`);
  }
  return problems;
}

// ---- Seeding ----

const botInput = (name: string, provider: BotInput["provider"], model: string): BotInput => ({
  name, color: "blue", shape: "circle", instructions: `You are ${name}. Answer briefly.`, provider, model, autonomy: "ask", browser: false,
});

// A 0.4.2 data root as the TypeScript daemon leaves it: accounts, bots with their folders, chats and recorded usage.
// Every message is finished and no work is queued, so a daemon's start-up recovery has nothing to change.
export function seedRoot(root: string): Counts {
  const dataDir = join(root, "data");
  const dbFile = join(dataDir, "tjommie.db");
  if (existsSync(dbFile)) throw new Error("Refusing to seed a data root that already has a database.");
  mkdirSync(dataDir, { recursive: true });
  const botsDir = join(root, "bots");
  mkdirSync(botsDir, { recursive: true });
  const db = openDb(dbFile);
  try {
    ensureAccounts(db, join(root, "accounts"), root);
    const bots = [
      createBot(db, botsDir, botInput("Writer", "claude", "haiku")),
      createBot(db, botsDir, botInput("Researcher", "claude", "sonnet")),
      createBot(db, botsDir, botInput("Coder", "codex", "gpt-5.3-codex")),
    ];
    for (const bot of bots) {
      for (let turn = 1; turn <= 5; turn++) {
        addMessage(db, { conversationId: bot.id, author: turn % 2 === 1 ? "you" : bot.id, kind: "text", body: `Seeded message ${turn} for ${bot.name}`, status: turn % 2 === 1 ? "sent" : "done" });
      }
      recordUsage(db, { botId: bot.id, accountId: `${bot.provider}-main`, provider: bot.provider, model: bot.model, input: 120, cachedInput: 40, output: 60, reasoning: 0, costUsd: 0.01, costSource: "reported" });
    }
  } finally {
    db.close(); // The last connection checkpoints the WAL, so the file is complete on its own.
  }
  return readCounts(dbFile, botsDir);
}

// ---- Verifying ----

export type Stage = "upgraded" | "rolledback";

// `upgraded`: after the Rust daemon first ran on the root. `rolledback`: after 0.4.2 ran on it again. Row counts and
// user_version never change (spec §5), and the database stays whole.
export function verifyRoot(root: string, stage: Stage, expected: Counts, version?: string): string[] {
  const problems: string[] = [];
  const dbFile = join(root, "data", "tjommie.db");
  if (!existsSync(dbFile)) return [`The database is missing: ${dbFile}`];
  if (stage === "upgraded") {
    const backupDir = join(root, "data", "backup");
    const backup = join(backupDir, `tjommie-${version ?? ""}.db`);
    if (!version) problems.push("The upgraded stage needs --version, the version of the daemon that ran first.");
    else if (!existsSync(backup)) problems.push(`No backup was made before the first open: ${backup}`);
    else {
      problems.push(...diffCounts("backup", expected, readCounts(backup)));
      const state = integrity(backup);
      if (state !== "ok") problems.push(`backup: integrity_check says ${state}`);
    }
    const partial = existsSync(backupDir) ? readdirSync(backupDir).filter((name) => name.endsWith(".partial")) : [];
    if (partial.length) problems.push(`A backup copy was left unfinished: ${partial.join(", ")}`);
  }
  const live = readCounts(dbFile, join(root, "bots"));
  problems.push(...diffCounts("live", expected, live));
  if (live.botFiles !== expected.botFiles) problems.push(`live: the bots folder has ${live.botFiles} files, expected ${expected.botFiles}`);
  const state = integrity(dbFile);
  if (state !== "ok") problems.push(`live: integrity_check says ${state}`);
  return problems;
}

// ---- Running an install ----

// The app NSIS installs for one user: %LOCALAPPDATA%\Programs\<product>\Tjommie.exe.
export function findInstalledApp(localAppData: string): string {
  const programs = join(localAppData, "Programs");
  const found = existsSync(programs) ? readdirSync(programs, { withFileTypes: true }).filter((entry) => entry.isDirectory() && existsSync(join(programs, entry.name, "Tjommie.exe"))) : [];
  if (found.length !== 1) throw new Error(`Expected exactly one Tjommie install under ${programs}, found ${found.length}.`);
  return join(programs, found[0].name);
}

export function stopTjommie(): void {
  spawnSync("taskkill", ["/F", "/T", "/IM", "Tjommie.exe"], { stdio: "ignore", windowsHide: true }); // Nothing running is fine.
}

const pause = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

// Installs silently over whatever is there. A silent NSIS install can return before its last files are written, so the
// folder counts as installed once its file count stops changing.
export async function installTjommie(installer: string, localAppData: string): Promise<string> {
  stopTjommie();
  const run = spawnSync(installer, ["/S"], { stdio: "inherit", windowsHide: true });
  if (run.error) throw run.error;
  if (run.status !== 0) throw new Error(`The installer failed (${run.status ?? run.signal}).`);
  let previous = -1;
  for (let attempt = 0; attempt < 30; attempt++) {
    await pause(2_000);
    let dir: string | undefined;
    try { dir = findInstalledApp(localAppData); } catch { dir = undefined; }
    if (dir) {
      const count = listFiles(dir).length;
      if (count === previous) { stopTjommie(); return dir; }
      previous = count;
    }
  }
  throw new Error("The install did not settle.");
}

// The daemon an install runs by itself: the bundled Rust exe, or 0.4.2's bundled Node daemon inside its own Electron.
export function installedDaemon(install: string): Pick<DaemonLaunch, "execPath" | "args"> {
  const rust = packagedDaemonPath(join(install, "resources"));
  if (existsSync(rust)) return { execPath: rust, args: [] };
  const entry = join(install, "resources", "daemon", "main.mjs");
  if (existsSync(entry)) return { execPath: join(install, "Tjommie.exe"), args: ["--disable-warning=ExperimentalWarning", entry] };
  throw new Error(`The install has no daemon: ${install}`);
}

// Starts the installed daemon on `root` as the app would (a free port, the token on stdin), waits until it listens, then
// stops it the way the app does: by closing its stdin.
export async function runInstalledDaemon(install: string, root: string): Promise<void> {
  const launch: DaemonLaunch = { ...installedDaemon(install), env: { ...process.env, TJOMMIE_HOME: root, TJOMMIE_PORT: "0" } };
  const running = await startDaemonProcess(launch, (text) => process.stdout.write(text), 120_000);
  await running.stop();
  const { code } = await running.exited;
  if (code !== 0) throw new Error(`The daemon stopped with exit code ${code}.`);
}

// ---- Command line ----

function option(args: string[], name: string): string | undefined {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? args[at + 1] : undefined;
}

function required(args: string[], name: string): string {
  const value = option(args, name);
  if (!value) throw new Error(`Missing --${name}.`);
  return value;
}

export async function main(args: string[]): Promise<void> {
  const [command, ...rest] = args;
  if (command !== "files") assertCi();
  switch (command) {
    case "seed": {
      const counts = seedRoot(resolve(required(rest, "root")));
      writeFileSync(required(rest, "counts"), `${JSON.stringify(counts, null, 2)}\n`);
      console.error(`Seeded ${Object.values(counts.tables).reduce((sum, rows) => sum + rows, 0)} rows in ${Object.keys(counts.tables).length} tables and ${counts.botFiles} bot files.`);
      return;
    }
    case "install": {
      const localAppData = process.env.LOCALAPPDATA;
      if (!localAppData) throw new Error("LOCALAPPDATA is not set.");
      console.log(await installTjommie(resolve(required(rest, "installer")), localAppData));
      return;
    }
    case "start":
      await runInstalledDaemon(resolve(required(rest, "install")), resolve(required(rest, "root")));
      return;
    case "verify": {
      const stage = required(rest, "stage");
      if (stage !== "upgraded" && stage !== "rolledback") throw new Error("--stage is upgraded or rolledback.");
      const expected = JSON.parse(readFileSync(required(rest, "counts"), "utf8")) as Counts;
      const problems = verifyRoot(resolve(required(rest, "root")), stage, expected, option(rest, "version"));
      if (problems.length) throw new Error(`The ${stage} check failed:\n${problems.map((problem) => `- ${problem}`).join("\n")}`);
      console.error(`The ${stage} check passed: ${Object.keys(expected.tables).length} tables, user_version ${expected.userVersion}.`);
      return;
    }
    case "files": {
      const files = listFiles(resolve(required(rest, "dir")), rest.includes("--installed"));
      console.log(files.length);
      const same = option(rest, "same-as");
      if (same) {
        const differences = diffFiles(listFiles(resolve(same)), files);
        if (differences.length) throw new Error(`The file list differs from ${same} (${differences.length} differences):\n${differences.slice(0, 40).join("\n")}`);
      }
      const equals = option(rest, "equals");
      if (equals !== undefined && files.length !== Number(equals)) throw new Error(`Expected ${equals} files, found ${files.length}.`);
      return;
    }
    default:
      throw new Error("Usage: r1-acceptance.ts seed|install|start|verify|files (see the header of this file).");
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main(process.argv.slice(2)).catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "The R1 acceptance step failed.");
    process.exitCode = 1;
  });
}
