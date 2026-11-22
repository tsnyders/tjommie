import { spawnSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { dirname, join, relative, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import { type Counts, assertCi, diffCounts, readCounts, runInstalledDaemon, seedRoot } from "./r1-acceptance.ts";

// The Phase 3 acceptance run on a runner with R1 installed (plan T8.4, .github/workflows/desktop-acceptance.yml). The PowerShell scripts in installer/
// (accept-update-path.ps1, accept-production-over-r1.ps1) install, update and roll back; this is the part that is easier here: seeding a data root,
// driving R1's own updater and exporter, and comparing a data root with what it held before.
//
//   seed       --root <dir> --install <R1 dir>                       a data root from the committed database (R1's own daemon starts on it once) and
//                                                                     the files R1's browser keeps, which must survive the new app and a rollback
//   snapshot   --root <dir> --out <file>                             the row counts, user_version and every file's SHA-256, as the baseline
//   verify     --root <dir> --baseline <file> [--report <file>]      nothing in the baseline is gone or changed; only known additions (see below);
//              [--allow-decrease <table,...>]                         a table may only shrink if it is named here
//   notice     --root <dir>                                          how many routine-failure notices are still pending (JSON)
//   r1-update  --install <dir> --feed <dir> --root <dir> --user-data <dir> --baseline <file> --expect-version <v>   (the update-test R1)
//   r1-export  --install <dir> --fixture <file>                       (production R1) sets the appearance and paired PCs in its page, and waits for its
//                                                                     export file to say what they are
//
// Everything that changes the machine refuses to run anywhere but a GitHub-hosted runner (r1-acceptance.ts assertCi). This is never run on a real PC.

export type Tree = Record<string, string>;
export type Baseline = { counts: Counts; tree: Tree };

// ---- the data root ----------------------------------------------------------------------------------------------------------------

const walk = (root: string): string[] =>
  readdirSync(root, { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? walk(join(root, entry.name)) : entry.isFile() ? [join(root, entry.name)] : []));

// Every file under `root` as its relative path (forward slashes, lower case) and SHA-256; "unreadable" for one another process holds open.
export function treeOf(root: string): Tree {
  const tree: Tree = {};
  if (!existsSync(root)) return tree;
  for (const file of walk(root).sort()) {
    let hash = "unreadable";
    try { hash = createHash("sha256").update(readFileSync(file)).digest("hex"); } catch { /* a file a running program holds */ }
    tree[relative(root, file).replaceAll("\\", "/").toLowerCase()] = hash;
  }
  return tree;
}

// What a new app version may do to a data root that an older one used. Nothing in the baseline may be gone (except files that only exist while a
// program runs), nothing the user's own data (bots, accounts, the browser's profiles, sentinels) may change a byte, and what is added has to be
// in a folder the apps write to. Anything else is a problem that names the file: a surprise is never waved through.
export type TreePolicy = { runtimeOnly: RegExp[]; contentKept: RegExp[]; allowedAdditions: RegExp[] };
export const DATA_ROOT_POLICY: TreePolicy = {
  // Files that exist only while a program runs, or that a running daemon rewrites as a matter of course.
  runtimeOnly: [/^daemon-token/, /\.lock$/, /(^|\/)lockfile$/, /-(wal|shm)$/, /^logs\//, /^data\/tjommie\.db$/],
  contentKept: [/^bots\//, /^accounts\//, /^data\/native-browser\//, /^sentinel\//],
  // data/logins.sqlite is the saved-logins vault's database, which R1 and the new app both open: its WAL and shared-memory files come and go with the
  // process that has it open (run 2: both appeared at the new app's start and again at R1's, and nothing else did).
  allowedAdditions: [/^logs\//, /^data\/backup\//, /^data\/desktop-ui\.json$/, /^data\/tjommie\.db(-wal|-shm)?$/, /^data\/logins\.sqlite(-wal|-shm)?$/, /^daemon-token/, /\.lock$/, /(^|\/)lockfile$/],
};

export function compareTrees(before: Tree, after: Tree, policy: TreePolicy = DATA_ROOT_POLICY): { problems: string[]; added: string[]; changedAppFiles: string[] } {
  const matches = (patterns: RegExp[], path: string): boolean => patterns.some((pattern) => pattern.test(path));
  const problems: string[] = [];
  const changedAppFiles: string[] = [];
  for (const [path, hash] of Object.entries(before)) {
    const now = after[path];
    if (now === undefined) {
      if (!matches(policy.runtimeOnly, path)) problems.push(`missing: ${path}`);
    } else if (hash !== "unreadable" && now !== "unreadable" && hash !== now) {
      if (matches(policy.contentKept, path)) problems.push(`changed: ${path}`);
      else changedAppFiles.push(path);
    }
  }
  const added = Object.keys(after).filter((path) => before[path] === undefined);
  for (const path of added) if (!matches(policy.allowedAdditions, path)) problems.push(`unexpected addition: ${path}`);
  return { problems, added, changedAppFiles };
}

// The row counts and user_version of a root, against a baseline's. A table may shrink only if the caller names it (the pending notice the new app
// acknowledges), and then only by shrinking: a missing table, a new one or more rows are still differences.
export function compareCounts(label: string, expected: Counts, found: Counts, allowDecrease: string[] = []): string[] {
  const all = diffCounts(label, expected, found);
  return all.filter((line) => {
    const table = /table (\S+) has (\d+) rows, expected (\d+)/.exec(line);
    return !(table && allowDecrease.includes(table[1]) && Number(table[2]) < Number(table[3]));
  });
}

export function snapshotRoot(root: string): Baseline {
  const dbFile = join(root, "data", "tjommie.db");
  if (!existsSync(dbFile)) throw new Error(`The database is missing: ${dbFile}`);
  return { counts: readCounts(dbFile, join(root, "bots")), tree: treeOf(root) };
}

const readBaseline = (file: string): Baseline => JSON.parse(readFileSync(file, "utf8")) as Baseline;

// ---- seeding ----------------------------------------------------------------------------------------------------------------------

// What R1's Electron browsers keep per bot, as files of known content: the new app starts fresh browsers of its own, and a rollback to R1 must
// find these exactly as they were (plan, Rollback).
export function seedBrowserProfiles(root: string): string[] {
  const made: string[] = [];
  for (const relativePath of ["data/native-browser/acceptance-bot/Default/Preferences", "data/native-browser/acceptance-bot/Default/Cookies", "data/native-browser/acceptance-bot/Default/Local Storage/leveldb/000003.log"]) {
    const file = join(root, ...relativePath.split("/"));
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, randomBytes(2048));
    made.push(relativePath);
  }
  return made;
}

// A routine-failure notice the daemon keeps until a window acknowledges it (scheduler.rs): a disabled routine, its failed run, and the notice.
// The new app fetches it when it starts, shows a toast, and the daemon deletes the row only after the OS has taken the toast.
export function plantPendingNotice(dbFile: string): void {
  const db = new DatabaseSync(dbFile);
  try {
    db.exec("PRAGMA busy_timeout = 15000");
    const bot = db.prepare("SELECT id FROM bots ORDER BY rowid LIMIT 1").get() as { id: string } | undefined;
    if (!bot) throw new Error("The database has no bot to hang a routine on.");
    const now = new Date().toISOString();
    db.prepare("INSERT INTO routines (id, bot_id, name, prompt, days, times, time_zone, time_limit_minutes, enabled, next_run_at, created_at) VALUES (?, ?, ?, ?, '[]', '[]', 'UTC', NULL, 0, NULL, ?)")
      .run("acceptance-routine", bot.id, "Acceptance routine", "Planted by the acceptance run; never runs.", now);
    db.prepare("INSERT INTO routine_runs (id, routine_id, due_at, started_at, ended_at, status, note) VALUES (?, ?, ?, ?, ?, 'failed', ?)")
      .run("acceptance-run", "acceptance-routine", now, now, now, "Planted by the acceptance run.");
    db.prepare("INSERT INTO routine_failure_notifications (run_id, title, body) VALUES (?, ?, ?)")
      .run("acceptance-run", "Acceptance's routine failed", "Acceptance routine: planted by the acceptance run.");
  } finally {
    db.close();
  }
}

export function pendingNotices(dbFile: string): number {
  const db = new DatabaseSync(dbFile);
  try {
    db.exec("PRAGMA busy_timeout = 15000");
    return (db.prepare("SELECT COUNT(*) AS n FROM routine_failure_notifications").get() as { n: number }).n;
  } finally {
    db.close();
  }
}

// ---- the local update feed --------------------------------------------------------------------------------------------------------

// electron-updater's generic provider: latest.yml and the installer, with range requests, from 127.0.0.1 only. Only the files that are in the feed
// folder when it starts are served, by exact name.
export async function startFeed(folder: string): Promise<{ server: Server; url: string }> {
  const allowed = new Set(readdirSync(folder));
  const server = createServer((request, response) => {
    const name = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname.slice(1));
    if (!allowed.has(name)) { response.writeHead(404).end(); return; }
    const file = join(folder, name);
    const size = statSync(file).size;
    const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range ?? "");
    const start = range ? Number(range[1]) : 0;
    const end = range && range[2] ? Number(range[2]) : size - 1;
    if (start > end || end >= size) { response.writeHead(416).end(); return; }
    response.writeHead(range ? 206 : 200, { "accept-ranges": "bytes", "content-length": end - start + 1, ...(range ? { "content-range": `bytes ${start}-${end}/${size}` } : {}) });
    createReadStream(file, { start, end }).pipe(response);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("The local update feed did not bind a TCP port.");
  return { server, url: `http://127.0.0.1:${address.port}` };
}

// ---- driving R1 -------------------------------------------------------------------------------------------------------------------

const pause = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));

// The promise's own result, or an error after `ms`; the timer is cleared either way, so a late timeout can never reject on its own.
export async function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const limit = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(message)), ms); });
  try { return await Promise.race([promise, limit]); } finally { if (timer) clearTimeout(timer); }
}

export async function eventually<T>(what: string, timeoutMs: number, probe: () => Promise<T | undefined | false> | T | undefined | false, intervalMs = 500): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const found = await probe();
    if (found !== undefined && found !== false) return found;
    if (Date.now() > deadline) throw new Error(`Timed out after ${Math.round(timeoutMs / 1000)} s waiting for ${what}.`);
    await pause(intervalMs);
  }
}

// The environment the update-test R1 is launched with, and the new app inherits it (R1 starts the installer, which starts the app). The update-test
// identity is a disposable one: its data root is TJOMMIE_HOME, and it starts the bundled daemon only if it is HANDED the daemon's path
// (live.rs daemon_exe takes the bundled one for the production identity alone; otherwise TJOMMIE_DAEMON_EXE, else a debug build beside the exe).
// Run 2 of the acceptance: without it the new app had no daemon to start, so no process, and a notice that stayed pending. R1 itself runs the daemon
// bundled with it whatever the environment says, so the variable changes nothing for R1; it is what the new app finds.
export function r1UpdateEnv(base: NodeJS.ProcessEnv, install: string, root: string, userData: string, feedUrl: string): Record<string, string> {
  // Playwright takes strings only: a variable the runner does not have is left out, not passed as "undefined".
  const inherited: Record<string, string> = {};
  for (const [name, value] of Object.entries(base)) if (value !== undefined) inherited[name] = value;
  return {
    ...inherited,
    TJOMMIE_HOME: root,
    TJOMMIE_USER_DATA: userData,
    TJOMMIE_ACCOUNTS_DIR: join(root, "accounts"),
    TJOMMIE_PORT: "0",
    TJOMMIE_UPDATE_FEED: feedUrl,
    TJOMMIE_BROWSER_HEADLESS: "1",
    TJOMMIE_DAEMON_EXE: join(install, "resources", "daemon", "tjommie-daemon.exe"),
  };
}

type UpdateState = { status: string; version?: string; message?: string; percent?: number };
type Bridge = { tjommie: { updates: { state(): Promise<UpdateState>; check(): Promise<void>; restart(): void } } };

const daemonListening = /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/;

function killTree(pid: number | undefined): void {
  if (pid) spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
}

// R1's update-test app updates itself, for real, from a feed on this machine: it downloads the Phase 3 update-test installer, and Restart to update
// quits it and runs that installer silently (`--updated /S --force-run`), which is the mechanism plan T8.4 step 1 asks to prove. The baseline is
// taken last, once the update is downloaded and ready, with the pending notice planted: that is what the new app must keep.
async function r1Update(args: string[]): Promise<void> {
  const install = resolve(required(args, "install"));
  const root = resolve(required(args, "root"));
  const userData = resolve(required(args, "user-data"));
  const feed = await startFeed(resolve(required(args, "feed")));
  const { _electron } = await import("playwright-core");
  const exe = join(install, "Tjommie Update Test.exe");
  const app = await _electron.launch({
    executablePath: exe,
    args: ["--hidden"],
    env: r1UpdateEnv(process.env, install, root, userData, feed.url),
  });
  const mainPid = app.process().pid;
  try {
    const page = await withTimeout(app.firstWindow(), 60_000, "R1's update-test app did not open a window within 60 seconds.");
    await page.waitForLoadState("load");
    const log = join(root, "logs", "desktop.log");
    await eventually("R1's daemon to listen", 120_000, () => (existsSync(log) && daemonListening.test(readFileSync(log, "utf8")) ? true : undefined));
    console.error("R1's update-test app is up on its data root.");
    await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.check());
    const expected = required(args, "expect-version");
    const ready = await eventually("the update to download", 240_000, async () => {
      const state = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.state());
      if (state.status === "error") throw new Error(`R1's updater reported an error: ${state.message ?? "(no message)"}`);
      return state.status === "ready" ? state : undefined;
    }, 1_000);
    if (ready.version !== expected) throw new Error(`R1 downloaded ${ready.version ?? "(no version)"}, expected ${expected}.`);
    console.error(`R1 downloaded ${ready.version}.`);
    plantPendingNotice(join(root, "data", "tjommie.db"));
    writeFileSync(required(args, "baseline"), `${JSON.stringify(snapshotRoot(root), null, 2)}\n`);
    console.error("Baseline written; asking R1 to restart to update.");
    const closed = new Promise<void>((done) => app.once("close", () => done()));
    await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.restart());
    await withTimeout(closed, 120_000, "R1 did not quit within 120 seconds of Restart to update.");
    console.error("R1 quit; its installer hand-over has started.");
  } catch (error) {
    killTree(mainPid);
    throw error;
  } finally {
    await new Promise<void>((done) => feed.server.close(() => done()));
  }
}

// Production R1 writes <root>\data\desktop-ui-export.json from its page's localStorage when the page loads (T1.6). The page here is given the
// fixture's values (the same ones the Rust side's fixture test reads), reloaded, and the file must then say exactly the fixture. That file is what
// the new app imports.
async function r1Export(args: string[]): Promise<void> {
  const install = resolve(required(args, "install"));
  const fixture = JSON.parse(readFileSync(resolve(required(args, "fixture")), "utf8")) as { theme: string; textSize: string; density: string; hosts: unknown[]; pending: unknown[] };
  const local = process.env.LOCALAPPDATA;
  if (!local) throw new Error("LOCALAPPDATA is not set.");
  const exported = join(local, "Tjommie", "data", "desktop-ui-export.json");
  const { _electron } = await import("playwright-core");
  const app = await _electron.launch({ executablePath: join(install, "Tjommie.exe"), args: ["--hidden"], env: { ...process.env, TJOMMIE_BROWSER_HEADLESS: "1" } });
  const mainPid = app.process().pid;
  try {
    const page = await withTimeout(app.firstWindow(), 60_000, "R1 did not open a window within 60 seconds.");
    await page.waitForLoadState("load");
    const stored: [string, string][] = [
      ["tjommie.theme", fixture.theme], ["tjommie.textSize", fixture.textSize], ["tjommie.density", fixture.density],
      ["tjommie.mobile.hosts.v1", JSON.stringify(fixture.hosts)], ["tjommie.mobile.pending.v1", JSON.stringify(fixture.pending)],
    ];
    // The page's own localStorage; the scripts' TypeScript has no DOM types, so it is reached through globalThis, as R1's own tests reach the bridge.
    await page.evaluate((pairs) => {
      const storage = (globalThis as unknown as { localStorage: { setItem(key: string, value: string): void } }).localStorage;
      for (const [key, value] of pairs) storage.setItem(key, value);
    }, stored);
    await page.reload();
    const written = await eventually("R1's export file to carry the fixture's values", 90_000, () => {
      if (!existsSync(exported)) return undefined;
      const text = readFileSync(exported, "utf8");
      return JSON.stringify(JSON.parse(text)) === JSON.stringify({ version: 1, ...fixture }) ? text : undefined;
    }, 1_000);
    console.error(`R1 wrote its export: ${Buffer.byteLength(written)} bytes, ${fixture.hosts.length} paired PCs and ${fixture.pending.length} pending.`);
    await Promise.race([app.close(), pause(30_000)]);
  } finally {
    killTree(mainPid);
  }
}

// ---- command line -----------------------------------------------------------------------------------------------------------------

function option(args: string[], name: string): string | undefined {
  const at = args.indexOf(`--${name}`);
  return at >= 0 ? args[at + 1] : undefined;
}
function required(args: string[], name: string): string {
  const value = option(args, name);
  if (!value) throw new Error(`Missing --${name}.`);
  return value;
}

export async function main(argv: string[]): Promise<void> {
  const [command, ...args] = argv;
  if (["seed", "r1-update", "r1-export"].includes(command ?? "")) assertCi();
  switch (command) {
    case "seed": {
      const install = resolve(required(args, "install"));
      const root = resolve(required(args, "root"));
      const counts = await seedRoot(root, (target) => runInstalledDaemon(install, target));
      const profiles = seedBrowserProfiles(root);
      console.error(`Seeded ${Object.keys(counts.tables).length} tables (user_version ${counts.userVersion}), ${counts.botFiles} bot files and ${profiles.length} browser-profile files.`);
      return;
    }
    case "snapshot": {
      const baseline = snapshotRoot(resolve(required(args, "root")));
      writeFileSync(required(args, "out"), `${JSON.stringify(baseline, null, 2)}\n`);
      console.error(`Baseline: user_version ${baseline.counts.userVersion}, ${Object.keys(baseline.counts.tables).length} tables, ${Object.values(baseline.counts.tables).reduce((sum, rows) => sum + rows, 0)} rows, ${Object.keys(baseline.tree).length} files.`);
      return;
    }
    case "verify": {
      const root = resolve(required(args, "root"));
      const before = readBaseline(required(args, "baseline"));
      const now = snapshotRoot(root);
      const allow = (option(args, "allow-decrease") ?? "").split(",").filter(Boolean);
      const trees = compareTrees(before.tree, now.tree);
      const problems = [...compareCounts("data root", before.counts, now.counts, allow), ...trees.problems];
      const report = {
        userVersion: { before: before.counts.userVersion, after: now.counts.userVersion },
        rows: { before: Object.values(before.counts.tables).reduce((sum, rows) => sum + rows, 0), after: Object.values(now.counts.tables).reduce((sum, rows) => sum + rows, 0) },
        tables: Object.fromEntries(Object.keys({ ...before.counts.tables, ...now.counts.tables }).map((name) => [name, { before: before.counts.tables[name] ?? null, after: now.counts.tables[name] ?? null }])),
        files: { before: Object.keys(before.tree).length, after: Object.keys(now.tree).length },
        added: trees.added,
        changedAppFiles: trees.changedAppFiles,
        problems,
      };
      const out = option(args, "report");
      if (out) writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`);
      console.log(JSON.stringify(report));
      if (problems.length) throw new Error(`The data root is not as the baseline left it:\n${problems.map((problem) => `- ${problem}`).join("\n")}`);
      return;
    }
    case "notice":
      console.log(JSON.stringify({ pending: pendingNotices(join(resolve(required(args, "root")), "data", "tjommie.db")) }));
      return;
    case "r1-update":
      await r1Update(args);
      return;
    case "r1-export":
      await r1Export(args);
      return;
    default:
      throw new Error("Usage: phase3-acceptance.ts seed|snapshot|verify|notice|r1-update|r1-export (see the header of this file).");
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main(process.argv.slice(2)).catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : "The Phase 3 acceptance step failed.");
    process.exitCode = 1;
  });
}
