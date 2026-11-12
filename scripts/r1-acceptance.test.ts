import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, expect, it, vi } from "vitest";
import { migrations } from "../packages/daemon/src/db.ts";
import { assertCi, diffFiles, findInstalledApp, installedDaemon, listFiles, main, readCounts, seedRoot, verifyRoot } from "./r1-acceptance.ts";

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

// A seeded root with the backup the daemon's first open would make.
function seededWithBackup(version = "9.9.9") {
  const root = temp();
  const expected = seedRoot(root);
  mkdirSync(join(root, "data", "backup"), { recursive: true });
  copyFileSync(join(root, "data", "tjommie.db"), join(root, "data", "backup", `tjommie-${version}.db`));
  return { root, expected, dbFile: join(root, "data", "tjommie.db"), backup: join(root, "data", "backup", `tjommie-${version}.db`) };
}
function sql(file: string, statement: string): void {
  const db = new DatabaseSync(file);
  try { db.exec(statement); } finally { db.close(); }
}

it("seeds a 0.4.2 data root with accounts, bots and their folders, chats and usage", () => {
  const root = temp();
  const counts = seedRoot(root);
  expect(counts.userVersion).toBe(migrations.length);
  expect(counts.tables).toMatchObject({ accounts: 2, bots: 3, conversations: 3, messages: 15, usage: 3 });
  expect(counts.botFiles).toBe(3); // One notes.md in each bot's folder.
  expect(readCounts(join(root, "data", "tjommie.db"), join(root, "bots"))).toEqual(counts);
});

it("refuses to seed over an existing database", () => {
  const root = temp();
  seedRoot(root);
  expect(() => seedRoot(root)).toThrow("already has a database");
});

it("accepts a root whose backup and live database match the seed, at both stages", () => {
  const { root, expected } = seededWithBackup();
  expect(verifyRoot(root, "upgraded", expected, "9.9.9")).toEqual([]);
  expect(verifyRoot(root, "rolledback", expected)).toEqual([]);
});

it("demands the backup the first open should have made", () => {
  const { root, expected } = seededWithBackup();
  expect(verifyRoot(root, "upgraded", expected, "1.2.3").join("\n")).toContain("No backup was made");
  expect(verifyRoot(root, "upgraded", expected).join("\n")).toContain("needs --version");
  // The rollback stage doesn't look for a backup.
  expect(verifyRoot(root, "rolledback", expected)).toEqual([]);
});

it("reports a changed row count in the live database, a changed user_version and a lost bot file", () => {
  const { root, expected, dbFile } = seededWithBackup();
  sql(dbFile, "DELETE FROM messages WHERE id = (SELECT id FROM messages LIMIT 1)");
  sql(dbFile, "PRAGMA user_version = 13");
  rmSync(join(root, "bots", readCountsBotFolder(root), "notes.md"));
  const problems = verifyRoot(root, "upgraded", expected, "9.9.9").join("\n");
  expect(problems).toContain("live: table messages has 14 rows, expected 15");
  expect(problems).toContain("live: user_version is 13");
  expect(problems).toContain("live: the bots folder has 2 files, expected 3");
  expect(problems).not.toContain("backup:");
});

it("reports a backup that does not match the seed, and an unfinished backup copy", () => {
  const { root, expected, backup } = seededWithBackup();
  sql(backup, "DELETE FROM usage");
  put(join(root, "data", "backup", "tjommie-9.9.9.db.partial"));
  const problems = verifyRoot(root, "upgraded", expected, "9.9.9").join("\n");
  expect(problems).toContain("backup: table usage has 0 rows, expected 3");
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
  expect(installedDaemon(install)).toEqual({ execPath: join(install, "Tjommie.exe"), args: ["--disable-warning=ExperimentalWarning", join(install, "resources", "daemon", "main.mjs")] });
  put(join(install, "resources", "daemon", "tjommie-daemon.exe"));
  expect(installedDaemon(install)).toEqual({ execPath: join(install, "resources", "daemon", "tjommie-daemon.exe"), args: [] });
});

it("changes a machine only on a GitHub Actions runner", async () => {
  expect(() => assertCi({})).toThrow("only on a GitHub Actions runner");
  expect(() => assertCi({ GITHUB_ACTIONS: "false" })).toThrow("only on a GitHub Actions runner");
  expect(() => assertCi({ GITHUB_ACTIONS: "true" })).not.toThrow();
  vi.stubEnv("GITHUB_ACTIONS", "");
  const root = temp();
  for (const command of [["seed", "--root", root, "--counts", join(root, "counts.json")], ["install", "--installer", join(root, "x.exe")], ["start", "--install", root, "--root", root], ["verify", "--root", root, "--counts", join(root, "c.json"), "--stage", "upgraded"]]) {
    await expect(main(command)).rejects.toThrow("only on a GitHub Actions runner");
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
