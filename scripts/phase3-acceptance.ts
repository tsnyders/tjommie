import { spawnSync } from "node:child_process";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { userInfo } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import { VaultRecords, WindowsLoginCredentials, type VaultCredentials } from "../packages/desktop/src/vault-storage.ts";
import { type Counts, assertCi, diffCounts, hostedRunnerProblems, readCounts, runInstalledDaemon, seedRoot } from "./r1-acceptance.ts";

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
//
// The saved-logins vault is part of the data root and of every comparison: seed puts invented logins in it (R1's own VaultRecords for the records,
// R1's own Credential Manager reader/writer for the secrets), snapshot and verify hold its records and secrets to the baseline (see "the saved-logins vault").

export type Tree = Record<string, string>;
export type Baseline = { counts: Counts; tree: Tree; vault: VaultSnapshot | null };

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

export async function snapshotRoot(root: string, credentials?: Pick<VaultCredentials, "read">): Promise<Baseline> {
  const dbFile = join(root, "data", "tjommie.db");
  if (!existsSync(dbFile)) throw new Error(`The database is missing: ${dbFile}`);
  const counts = readCounts(dbFile, join(root, "bots"));
  const vault = await snapshotVault(root, credentials);
  return { counts, tree: treeOf(root), vault };
}

// A baseline written before the vault was part of it has no `vault`, which reads as "no vault then".
const readBaseline = (file: string): Baseline => {
  const parsed = JSON.parse(readFileSync(file, "utf8")) as Omit<Baseline, "vault"> & { vault?: VaultSnapshot | null };
  return { ...parsed, vault: parsed.vault ?? null };
};

// ---- the saved-logins vault -------------------------------------------------------------------------------------------------------

// data/logins.sqlite holds each saved login's metadata and the reference of its secret; the secret itself (username and password) is a Credential Manager
// entry named by that reference, in the namespace of the data root. R1 and the new app share both. A person who upgrades must keep every saved login, and a
// rollback to R1 must find them, so the baseline holds the records exactly and the secrets by SHA-256 (the baseline is uploaded as evidence; a secret is
// never written down, not even an invented one).
//
// What can be read back is whatever R1's own reader (WindowsLoginCredentials) can read on this runner. ci.yml and r1-acceptance.yml say hosted service logons
// cannot reliably reach Credential Manager, so a secret the runner would not take is recorded as not readable (`secret: null`) and is not held to anything:
// the report says how many were, and the PowerShell side notes it instead of passing it as a check.
export type VaultLogin = { id: string; reference: string; data: string; secret: string | null };
export type VaultSnapshot = { userVersion: number; logins: VaultLogin[]; deletions: string[] };

// The stored entry as JSON with its keys in a fixed order, so a rewrite that only reorders keys or spacing is not a change.
const canonicalJson = (text: string): string => {
  const sorted = (value: unknown): unknown =>
    Array.isArray(value) ? value.map(sorted)
      : value !== null && typeof value === "object" ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([key, inner]) => [key, sorted(inner)]))
        : value;
  return JSON.stringify(sorted(JSON.parse(text)));
};

// Where a snapshot reads secrets from when it is not handed a reader. `snapshot` and `verify` have no runner guard, so that they can still count and compare
// a data root by hand; but a secret is read, and written down as a hash, only on a GitHub-hosted runner, where every login is one this run invented. Anywhere
// else a hash of a real person's password would land in a file, so there the secrets are not read at all (every one is "not readable", and says so).
export function credentialsFor(root: string, env: NodeJS.ProcessEnv = process.env, user: string = userInfo().username): Pick<VaultCredentials, "read"> {
  return hostedRunnerProblems(env, user).length === 0 ? new WindowsLoginCredentials(root) : { read: async () => null };
}

async function secretHash(credentials: Pick<VaultCredentials, "read">, reference: string): Promise<string | null> {
  try {
    const secret = await credentials.read(reference);
    return secret ? createHash("sha256").update(JSON.stringify([secret.username, secret.password])).digest("hex") : null;
  } catch {
    return null; // the runner's Credential Manager would not give it up: not readable, which is not the same as gone
  }
}

// The vault as it is now, or null when the root has none. Read-only: this runs against whatever app has the file open, and by hand it must never be able to write.
export async function snapshotVault(root: string, credentials: Pick<VaultCredentials, "read"> = credentialsFor(root)): Promise<VaultSnapshot | null> {
  const file = join(root, "data", "logins.sqlite");
  if (!existsSync(file)) return null;
  const db = new DatabaseSync(file, { readOnly: true });
  let userVersion: number;
  let rows: { id: string; data: string; reference: string }[];
  let deletions: string[];
  try {
    db.exec("PRAGMA busy_timeout = 15000");
    userVersion = (db.prepare("PRAGMA user_version").get() as { user_version: number }).user_version;
    rows = db.prepare("SELECT id, data, reference FROM logins ORDER BY id").all() as { id: string; data: string; reference: string }[];
    deletions = (db.prepare("SELECT reference FROM login_deletions ORDER BY reference").all() as { reference: string }[]).map((row) => row.reference);
  } finally {
    db.close();
  }
  const logins: VaultLogin[] = [];
  for (const row of rows) logins.push({ id: row.id, reference: row.reference, data: canonicalJson(row.data), secret: await secretHash(credentials, row.reference) });
  return { userVersion, logins, deletions };
}

// What differs between the vault at the baseline and now: a login gone, added or changed, its secret no longer readable (or different) when it was
// readable at the baseline, a pending secret deletion that was not there, and the vault's user_version. No secret and no hash is ever in a message.
export function compareVaults(before: VaultSnapshot | null, after: VaultSnapshot | null): string[] {
  if (before === null) return after && (after.logins.length > 0 || after.deletions.length > 0) ? [`vault: the baseline had no saved logins, and now there are ${after.logins.length}`] : [];
  if (after === null) return ["vault: data/logins.sqlite is gone"];
  const problems: string[] = [];
  if (after.userVersion !== before.userVersion) problems.push(`vault: user_version is ${after.userVersion}, expected ${before.userVersion}`);
  const now = new Map(after.logins.map((login) => [login.id, login]));
  for (const login of before.logins) {
    const found = now.get(login.id);
    if (!found) { problems.push(`vault: saved login ${login.id} is gone`); continue; }
    if (found.reference !== login.reference) problems.push(`vault: saved login ${login.id} now names another secret`);
    if (found.data !== login.data) problems.push(`vault: saved login ${login.id} changed`);
    if (login.secret !== null && found.secret !== login.secret) problems.push(`vault: the secret of saved login ${login.id} ${found.secret === null ? "can no longer be read" : "is not what it was"}`);
  }
  const had = new Set(before.logins.map((login) => login.id));
  for (const login of after.logins) if (!had.has(login.id)) problems.push(`vault: saved login ${login.id} is new`);
  if (after.deletions.join("|") !== before.deletions.join("|")) problems.push(`vault: the secrets waiting to be deleted are ${after.deletions.length}, were ${before.deletions.length}`);
  return problems;
}

export type VaultSummary = { logins: number; secretsReadable: number; pendingDeletions: number; userVersion: number };
export const summarizeVault = (vault: VaultSnapshot | null): VaultSummary | null =>
  vault && { logins: vault.logins.length, secretsReadable: vault.logins.filter((login) => login.secret !== null).length, pendingDeletions: vault.deletions.length, userVersion: vault.userVersion };

// Invented logins, so that the vault is not empty when the new app and the rollback meet it. The records are written by R1's own VaultRecords, the way R1
// saves a login (an id, a reference, then the entry), and each secret by R1's own Credential Manager writer, in this root's namespace.
const INVENTED_LOGINS = [
  { name: "Acceptance mail", origin: "https://mail.example.com", username: "acceptance-user-1" },
  { name: "Acceptance bank", origin: "https://bank.example.com", username: "acceptance-user-2" },
];

export type SeededVault = { logins: number; secretsWritten: number; secretsRefused: number };

export async function seedVault(root: string, credentials: Pick<VaultCredentials, "write"> = new WindowsLoginCredentials(root)): Promise<SeededVault> {
  const botId = (() => {
    const db = new DatabaseSync(join(root, "data", "tjommie.db"), { readOnly: true });
    try { return (db.prepare("SELECT id FROM bots ORDER BY rowid LIMIT 1").get() as { id: string } | undefined)?.id; } finally { db.close(); }
  })();
  if (!botId) throw new Error("The database has no bot to hang a saved login on.");
  const records = new VaultRecords(join(root, "data", "logins.sqlite"));
  let written = 0;
  try {
    for (const login of INVENTED_LOGINS) {
      const reference = randomUUID();
      const now = new Date().toISOString();
      records.saved({ id: randomUUID(), name: login.name, origin: login.origin, botId, revision: 1, createdAt: now, updatedAt: now }, reference);
      try {
        await credentials.write(reference, { username: login.username, password: randomBytes(18).toString("base64url") });
        written++;
      } catch { /* this runner's Credential Manager would not take it: counted below, and the baseline then holds the record alone */ }
    }
  } finally {
    records.close();
  }
  return { logins: INVENTED_LOGINS.length, secretsWritten: written, secretsRefused: INVENTED_LOGINS.length - written };
}

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

// Read-only: `notice` has no runner guard (it only looks), so by hand it must not be able to write to a live database.
export function pendingNotices(dbFile: string): number {
  const db = new DatabaseSync(dbFile, { readOnly: true });
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
// identity is a disposable one, so its data root is TJOMMIE_HOME. It is deliberately NOT handed a daemon: an installed update-test app runs the daemon
// bundled in its install folder (desktop_host::daemon_process::daemon_exe_for), and that is what the update path has to prove, so nothing here may
// name one. (Run 2 injected TJOMMIE_DAEMON_EXE to get past the app, which did not look in its install; the app was fixed instead.) Any
// TJOMMIE_DAEMON_EXE the runner might carry is removed too, whatever its case, so no inherited variable can supply a daemon.
export function r1UpdateEnv(base: NodeJS.ProcessEnv, root: string, userData: string, feedUrl: string): Record<string, string> {
  // Playwright takes strings only: a variable the runner does not have is left out, not passed as "undefined".
  const inherited: Record<string, string> = {};
  for (const [name, value] of Object.entries(base)) if (value !== undefined && name.toUpperCase() !== "TJOMMIE_DAEMON_EXE") inherited[name] = value;
  return {
    ...inherited,
    TJOMMIE_HOME: root,
    TJOMMIE_USER_DATA: userData,
    TJOMMIE_ACCOUNTS_DIR: join(root, "accounts"),
    TJOMMIE_PORT: "0",
    TJOMMIE_UPDATE_FEED: feedUrl,
    TJOMMIE_BROWSER_HEADLESS: "1",
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
    env: r1UpdateEnv(process.env, root, userData, feed.url),
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
    writeFileSync(required(args, "baseline"), `${JSON.stringify(await snapshotRoot(root), null, 2)}\n`);
    console.error("Baseline written; asking R1 to restart to update.");
    // The instant R1's process ended, taken in the close handler itself: the PowerShell side (U7) tells the new app's processes from R1's by whether they
    // started after it. (Run 6: the time this script returns is too late, the installer had already started the new app by then.)
    let quitAt = new Date();
    const closed = new Promise<void>((done) => app.once("close", () => { quitAt = new Date(); done(); }));
    await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.updates.restart());
    await withTimeout(closed, 120_000, "R1 did not quit within 120 seconds of Restart to update.");
    console.error(`R1 quit at ${quitAt.toISOString()}; its installer hand-over has started.`);
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
      const vault = await seedVault(root);
      console.error(`Seeded ${Object.keys(counts.tables).length} tables (user_version ${counts.userVersion}), ${counts.botFiles} bot files and ${profiles.length} browser-profile files.`);
      console.error(`Seeded the saved-logins vault: ${vault.logins} invented logins; their secrets written to Credential Manager: ${vault.secretsWritten}, refused by this runner: ${vault.secretsRefused}.`);
      return;
    }
    case "snapshot": {
      const baseline = await snapshotRoot(resolve(required(args, "root")));
      writeFileSync(required(args, "out"), `${JSON.stringify(baseline, null, 2)}\n`);
      console.error(`Baseline: user_version ${baseline.counts.userVersion}, ${Object.keys(baseline.counts.tables).length} tables, ${Object.values(baseline.counts.tables).reduce((sum, rows) => sum + rows, 0)} rows, ${Object.keys(baseline.tree).length} files.`);
      const vault = summarizeVault(baseline.vault);
      console.error(vault ? `The saved-logins vault: ${vault.logins} logins, ${vault.secretsReadable} of their secrets readable, ${vault.pendingDeletions} secrets waiting to be deleted.` : "No saved-logins vault in this root.");
      return;
    }
    case "verify": {
      const root = resolve(required(args, "root"));
      const before = readBaseline(required(args, "baseline"));
      const now = await snapshotRoot(root);
      const allow = (option(args, "allow-decrease") ?? "").split(",").filter(Boolean);
      const trees = compareTrees(before.tree, now.tree);
      const problems = [...compareCounts("data root", before.counts, now.counts, allow), ...trees.problems, ...compareVaults(before.vault, now.vault)];
      const report = {
        vault: { before: summarizeVault(before.vault), after: summarizeVault(now.vault) },
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
