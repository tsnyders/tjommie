import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, expect, it, vi } from "vitest";
import { assertCi, diffFiles, findInstalledApp, firstStartByOldDaemon, installedDaemon, listFiles, main, readCounts, seedRoot, verifyRoot } from "./r1-acceptance.ts";

const folders: string[] = [];
function temp(): string {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-r1-test-"));
  folders.push(folder);
  return folder;
}
function put(path: string, content = "x"): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}
afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
});

// A first start that adds nothing, for the tests that only care about the seed.
const noStart = async (): Promise<void> => {};

// What a daemon's first start on a root leaves behind: its own `attachmentHostId` setting (daemon.ts:159-161 in 0.4.2, m6.rs
// in the Rust daemon, which only inserts it when absent).
const bootstrap = (file: string): void => sql(file, "INSERT OR IGNORE INTO settings (key, value) VALUES ('attachmentHostId', 'host-1')");

// A seeded root with the backup the daemon's first open would make.
async function seededWithBackup(version = "9.9.9") {
  const root = temp();
  const expected = await seedRoot(root, noStart);
  mkdirSync(join(root, "data", "backup"), { recursive: true });
  copyFileSync(join(root, "data", "tjommie.db"), join(root, "data", "backup", `tjommie-${version}.db`));
  return { root, expected, dbFile: join(root, "data", "tjommie.db"), backup: join(root, "data", "backup", `tjommie-${version}.db`) };
}
function sql(file: string, statement: string): void {
  const db = new DatabaseSync(file);
  try { db.exec(statement); } finally { db.close(); }
}

it("seeds a 0.4.2 data root from the committed 0.4.2 database, its folders moved under the root", async () => {
  const root = temp();
  const counts = await seedRoot(root, noStart);
  expect(counts.userVersion).toBe(12); // 0.4.2's last migration, which the Rust daemon also ends at (spec §5).
  // Two conversations: the bot's, and the team room a migration creates.
  expect(counts.tables).toMatchObject({ accounts: 1, bots: 1, conversations: 2, messages: 4, approvals: 1, work_items: 2, sessions: 1 });
  expect(counts.botFiles).toBe(1); // One notes.md in the bot's folder.
  expect(readCounts(join(root, "data", "tjommie.db"), join(root, "bots"))).toEqual(counts);
  const db = new DatabaseSync(join(root, "data", "tjommie.db"));
  try {
    expect(db.prepare("SELECT home_folder AS folder FROM bots").all()).toEqual([{ folder: join(root, "bots", "legacy-bot") }]);
    expect(db.prepare("SELECT folder FROM accounts").all()).toEqual([{ folder: join(root, "accounts", "claude-main") }]);
    // Settled: a daemon's start-up recovery has nothing to change, and no queued work would start a provider CLI.
    expect(db.prepare("SELECT COUNT(*) AS n FROM messages WHERE status IN ('streaming', 'queued')").get()).toEqual({ n: 0 });
    expect(db.prepare("SELECT COUNT(*) AS n FROM work_items WHERE status IN ('running', 'queued')").get()).toEqual({ n: 0 });
    expect(db.prepare("SELECT COUNT(*) AS n FROM approvals WHERE status = 'pending'").get()).toEqual({ n: 0 });
  } finally { db.close(); }
});

it("never changes the committed database it copies", async () => {
  const fixture = new URL("../tjommie-rs/crates/daemon/tests/fixtures/fixture.db", import.meta.url);
  const before = readFileSync(fixture);
  await seedRoot(temp(), noStart);
  expect(readFileSync(fixture).equals(before)).toBe(true);
});

it("starts a daemon on the seeded root before it counts, so the counts include what that start adds", async () => {
  const root = temp();
  const started: { root: string; bots: number; settings: number }[] = [];
  const counts = await seedRoot(root, async (seeded) => {
    const before = readCounts(join(seeded, "data", "tjommie.db"));
    started.push({ root: seeded, bots: before.tables.bots, settings: before.tables.settings });
    bootstrap(join(seeded, "data", "tjommie.db"));
  });
  // The daemon sees the finished seed, not a half-built one, and starts exactly once.
  expect(started).toEqual([{ root, bots: 1, settings: 0 }]);
  expect(counts.tables).toMatchObject({ bots: 1, messages: 4, settings: 1 });
});

it("passes the upgrade check when R1 only finds the rows 0.4.2's first start left, and fails on them when nothing ever started", async () => {
  // The seed that ran 0.4.2's daemon: R1's own first start inserts nothing new (it only adds attachmentHostId when absent).
  const root = temp();
  const expected = await seedRoot(root, async (seeded) => bootstrap(join(seeded, "data", "tjommie.db")));
  mkdirSync(join(root, "data", "backup"), { recursive: true });
  copyFileSync(join(root, "data", "tjommie.db"), join(root, "data", "backup", "tjommie-9.9.9.db"));
  bootstrap(join(root, "data", "tjommie.db"));
  expect(verifyRoot(root, "upgraded", expected, "9.9.9")).toEqual([]);

  // The seed that never ran a daemon: the first start adds the setting, and the check can't tell that from lost data.
  const bare = await seededWithBackup();
  bootstrap(bare.dbFile);
  expect(verifyRoot(bare.root, "upgraded", bare.expected, "9.9.9").join("\n")).toContain("live: table settings has 1 rows, expected 0");
});

it("starts no daemon on a root it refuses to seed", async () => {
  const root = temp();
  await seedRoot(root, noStart);
  const start = vi.fn(noStart);
  await expect(seedRoot(root, start)).rejects.toThrow("already has a database");
  expect(start).not.toHaveBeenCalled();
});

it("seeds only through 0.4.2's own Node daemon, never an install that carries the Rust one", () => {
  const install = temp();
  expect(() => firstStartByOldDaemon(install)).toThrow("no daemon");
  put(join(install, "resources", "daemon", "main.mjs"));
  expect(typeof firstStartByOldDaemon(install)).toBe("function");
  put(join(install, "resources", "daemon", "tjommie-daemon.exe"));
  expect(() => firstStartByOldDaemon(install)).toThrow("runs the Rust one");
});

it("accepts a root whose backup and live database match the seed, at both stages", async () => {
  const { root, expected } = await seededWithBackup();
  expect(verifyRoot(root, "upgraded", expected, "9.9.9")).toEqual([]);
  expect(verifyRoot(root, "rolledback", expected)).toEqual([]);
});

it("demands the backup the first open should have made", async () => {
  const { root, expected } = await seededWithBackup();
  expect(verifyRoot(root, "upgraded", expected, "1.2.3").join("\n")).toContain("No backup was made");
  expect(verifyRoot(root, "upgraded", expected).join("\n")).toContain("needs --version");
  // The rollback stage doesn't look for a backup.
  expect(verifyRoot(root, "rolledback", expected)).toEqual([]);
});

it("reports a changed row count in the live database, a changed user_version and a lost bot file", async () => {
  const { root, expected, dbFile } = await seededWithBackup();
  sql(dbFile, "DELETE FROM messages WHERE id = (SELECT id FROM messages LIMIT 1)");
  sql(dbFile, "PRAGMA user_version = 13");
  rmSync(join(root, "bots", readCountsBotFolder(root), "notes.md"));
  const problems = verifyRoot(root, "upgraded", expected, "9.9.9").join("\n");
  expect(problems).toContain("live: table messages has 3 rows, expected 4");
  expect(problems).toContain("live: user_version is 13");
  expect(problems).toContain("live: the bots folder has 0 files, expected 1");
  expect(problems).not.toContain("backup:");
});

it("reports a backup that does not match the seed, and an unfinished backup copy", async () => {
  const { root, expected, backup } = await seededWithBackup();
  sql(backup, "DELETE FROM work_items");
  put(join(root, "data", "backup", "tjommie-9.9.9.db.partial"));
  const problems = verifyRoot(root, "upgraded", expected, "9.9.9").join("\n");
  expect(problems).toContain("backup: table work_items has 0 rows, expected 2");
  expect(problems).toContain("A backup copy was left unfinished");
});

it("reports a missing database", () => {
  const root = temp();
  expect(verifyRoot(root, "rolledback", { userVersion: 12, tables: {}, botFiles: 0 }).join("\n")).toContain("The database is missing");
});

it("counts an install's files without its NSIS uninstaller, and only the one beside the app", () => {
  const dir = temp();
  for (const path of ["Tjommie.exe", "resources/app.asar", "resources/daemon/tjommie-daemon.exe", "Uninstall Tjommie.exe"]) put(join(dir, path));
  expect(listFiles(dir)).toHaveLength(4);
  expect(listFiles(dir, true)).toEqual(["resources/app.asar", "resources/daemon/tjommie-daemon.exe", "tjommie.exe"]);
  put(join(dir, "resources", "Uninstall extra.exe"));
  expect(listFiles(dir, true)).toContain("resources/uninstall extra.exe");
});

it("names the files an install has too many or too few of", () => {
  expect(diffFiles(["a", "b"], ["b", "c"])).toEqual(["missing: a", "extra: c"]);
  expect(diffFiles(["a"], ["a"])).toEqual([]);
});

it("finds exactly one installed app under Programs", () => {
  const localAppData = temp();
  expect(() => findInstalledApp(localAppData)).toThrow("found 0");
  put(join(localAppData, "Programs", "Tjommie", "Tjommie.exe"));
  put(join(localAppData, "Programs", "Other", "Other.exe"));
  expect(findInstalledApp(localAppData)).toBe(join(localAppData, "Programs", "Tjommie"));
  put(join(localAppData, "Programs", "Tjommie RS", "Tjommie.exe"));
  expect(() => findInstalledApp(localAppData)).toThrow("found 2");
});

it("runs an install's bundled Rust daemon, else 0.4.2's bundled Node daemon inside its own Electron", () => {
  const install = temp();
  expect(() => installedDaemon(install)).toThrow("no daemon");
  put(join(install, "resources", "daemon", "main.mjs"));
  expect(installedDaemon(install)).toEqual({ execPath: join(install, "Tjommie.exe"), args: ["--disable-warning=ExperimentalWarning", join(install, "resources", "daemon", "main.mjs")], runAsNode: true });
  put(join(install, "resources", "daemon", "tjommie-daemon.exe"));
  expect(installedDaemon(install)).toEqual({ execPath: join(install, "resources", "daemon", "tjommie-daemon.exe"), args: [], runAsNode: false });
});

it("changes a machine only on a GitHub-hosted runner, not under act or on a self-hosted one", async () => {
  const refused = "only on a GitHub-hosted runner";
  expect(() => assertCi({})).toThrow(refused);
  expect(() => assertCi({ GITHUB_ACTIONS: "false", RUNNER_ENVIRONMENT: "github-hosted" })).toThrow(refused);
  // `act` and self-hosted runners set GITHUB_ACTIONS too, and either can be a PC with a real Tjommie on it.
  expect(() => assertCi({ GITHUB_ACTIONS: "true" })).toThrow(refused);
  expect(() => assertCi({ GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "self-hosted" })).toThrow(refused);
  expect(() => assertCi({ RUNNER_ENVIRONMENT: "github-hosted" })).toThrow(refused);
  expect(() => assertCi({ GITHUB_ACTIONS: "true", RUNNER_ENVIRONMENT: "github-hosted" })).not.toThrow();
  // Every stage that changes the machine checks it first. Cleared here so a real runner running this test still refuses.
  vi.stubEnv("GITHUB_ACTIONS", "");
  vi.stubEnv("RUNNER_ENVIRONMENT", "");
  const root = temp();
  for (const command of [["seed", "--root", root, "--install", root, "--counts", join(root, "counts.json")], ["install", "--installer", join(root, "x.exe")], ["start", "--install", root, "--root", root], ["verify", "--root", root, "--counts", join(root, "c.json"), "--stage", "upgraded"]]) {
    await expect(main(command)).rejects.toThrow(refused);
  }
});

it("prints a file count anywhere, and fails when it differs from an expected count or another folder", async () => {
  vi.stubEnv("GITHUB_ACTIONS", "");
  const fresh = temp();
  const installed = temp();
  for (const path of ["Tjommie.exe", "resources/app.asar"]) { put(join(fresh, path)); put(join(installed, path)); }
  put(join(installed, "Uninstall Tjommie.exe"));
  const log = vi.spyOn(console, "log").mockImplementation(() => {});
  await main(["files", "--dir", installed, "--installed", "--equals", "2", "--same-as", fresh]);
  expect(log).toHaveBeenCalledWith(2);
  await expect(main(["files", "--dir", installed, "--installed", "--equals", "3"])).rejects.toThrow("Expected 3 files, found 2");
  put(join(installed, "resources", "daemon", "main.mjs"));
  await expect(main(["files", "--dir", installed, "--installed", "--same-as", fresh])).rejects.toThrow("extra: resources/daemon/main.mjs");
});

// The folder name of the first seeded bot.
function readCountsBotFolder(root: string): string {
  const db = new DatabaseSync(join(root, "data", "tjommie.db"));
  try { return (db.prepare("SELECT id FROM bots ORDER BY created_at, rowid LIMIT 1").get() as { id: string }).id; } finally { db.close(); }
}
