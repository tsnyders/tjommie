import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DATA_ROOT_POLICY, compareCounts, compareTrees, eventually, pendingNotices, plantPendingNotice, r1UpdateEnv, seedBrowserProfiles, snapshotRoot, startFeed, treeOf, withTimeout,
} from "./phase3-acceptance.ts";

// The Phase 3 acceptance run's Node half (scripts/phase3-acceptance.ts). Nothing here starts R1, an installer or a daemon: the data-root rules are tried
// on temp folders, the notice seeding on a copy of the committed database (so its columns are held to the daemon's real schema), and the update feed on
// 127.0.0.1.

vi.setConfig({ testTimeout: 60_000 });

const repo = resolve(import.meta.dirname, "..");
const script = join(import.meta.dirname, "phase3-acceptance.ts");
const fixtureDb = join(repo, "tjommie-rs", "crates", "daemon", "tests", "fixtures", "fixture.db");

const folders: string[] = [];
const temp = (): string => {
  const folder = mkdtempSync(join(tmpdir(), "tjommie-phase3-test-"));
  folders.push(folder);
  return folder;
};
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

// A data root as R1 leaves one: the committed database, a bot folder, an account folder, a browser profile and a log.
function dataRoot(): string {
  const root = temp();
  mkdirSync(join(root, "data"), { recursive: true });
  cpSync(fixtureDb, join(root, "data", "tjommie.db"));
  for (const [path, text] of [["bots/legacy-bot/notes.md", "notes"], ["accounts/claude-main/state.json", "{}"], ["logs/desktop.log", "start\n"]]) {
    mkdirSync(join(root, ...path.split("/").slice(0, -1)), { recursive: true });
    writeFileSync(join(root, ...path.split("/")), text);
  }
  seedBrowserProfiles(root);
  return root;
}

describe("the data-root rules", () => {
  const tree = (entries: Record<string, string>): Record<string, string> => entries;

  it("accepts a root in which only known app-written things changed or arrived", () => {
    const before = tree({ "bots/a/notes.md": "1", "data/native-browser/b/Default/Preferences": "2", "data/tjommie.db": "3", "logs/desktop.log": "4", "daemon-token": "5" });
    const after = tree({
      ...before, "data/tjommie.db": "changed", "logs/desktop.log": "more", "logs/desktop-crash.log": "x", "data/backup/tjommie-0.6.0.db": "b", "data/desktop-ui.json": "j", "data/tjommie.db-wal": "w",
      "data/logins.sqlite-wal": "w", "data/logins.sqlite-shm": "s",
    });
    delete after["daemon-token"];
    const result = compareTrees(before, after);
    expect(result.problems).toEqual([]);
    expect(result.added.sort()).toEqual(["data/backup/tjommie-0.6.0.db", "data/desktop-ui.json", "data/logins.sqlite-shm", "data/logins.sqlite-wal", "data/tjommie.db-wal", "logs/desktop-crash.log"]);
    expect(result.changedAppFiles.sort()).toEqual(["data/tjommie.db", "logs/desktop.log"]);
  });

  it("names a file that is gone, a user file that changed, and an addition outside the known folders", () => {
    const before = tree({ "bots/a/notes.md": "1", "accounts/x/state.json": "2", "data/native-browser/b/Cookies": "3", "sentinel/notes.md": "4", "settings.json": "5" });
    const after = tree({ "bots/a/notes.md": "1", "accounts/x/state.json": "changed", "data/native-browser/b/Cookies": "3", "sentinel/notes.md": "4", "surprise/new.bin": "9" });
    expect(compareTrees(before, after).problems.sort()).toEqual(["changed: accounts/x/state.json", "missing: settings.json", "unexpected addition: surprise/new.bin"]);
    // Only the logins vault's own sidecars are allowed beside its database: another file in data/ is still a surprise.
    expect(compareTrees({}, { "data/logins.sqlite": "x", "data/logins.sqlite.bak": "y", "data/other.sqlite-wal": "z" }).problems.sort()).toEqual(["unexpected addition: data/logins.sqlite.bak", "unexpected addition: data/other.sqlite-wal"]);
    // A browser profile and a sentinel are held byte for byte too.
    const profile = compareTrees(tree({ "data/native-browser/b/Cookies": "3" }), tree({ "data/native-browser/b/Cookies": "4" }));
    expect(profile.problems).toEqual(["changed: data/native-browser/b/Cookies"]);
  });

  it("does not hold a file another program had open (unreadable) to its old content", () => {
    const result = compareTrees(tree({ "bots/a/notes.md": "unreadable" }), tree({ "bots/a/notes.md": "now readable" }));
    expect(result.problems).toEqual([]);
  });

  it("allows a table to shrink only when it is named, and nothing else to change", () => {
    const base = { userVersion: 12, tables: { bots: 1, routine_failure_notifications: 1 }, botFiles: 1 };
    const shrunk = { userVersion: 12, tables: { bots: 1, routine_failure_notifications: 0 }, botFiles: 1 };
    expect(compareCounts("x", base, shrunk, ["routine_failure_notifications"])).toEqual([]);
    expect(compareCounts("x", base, shrunk)).toHaveLength(1);
    const grown = { userVersion: 12, tables: { bots: 2, routine_failure_notifications: 1 }, botFiles: 1 };
    expect(compareCounts("x", base, grown, ["routine_failure_notifications"])).toHaveLength(1);
    expect(compareCounts("x", base, { ...grown, tables: { bots: 1, routine_failure_notifications: 2 } }, ["routine_failure_notifications"])).toHaveLength(1);
    expect(compareCounts("x", base, { ...base, userVersion: 13 })).toEqual(["x: user_version is 13, expected 12"]);
    // A table that is gone altogether is a difference even if it is the one allowed to shrink: only fewer rows are allowed, not no table.
    expect(compareCounts("x", base, { ...base, tables: { bots: 1 } }, ["routine_failure_notifications"])).toHaveLength(1);
  });

  it("reads a tree as relative lower-case forward-slash paths with each file's SHA-256, and an absent root as empty", () => {
    const root = dataRoot();
    const tree = treeOf(root);
    expect(tree["bots/legacy-bot/notes.md"]).toMatch(/^[0-9a-f]{64}$/);
    expect(Object.keys(tree).every((path) => path === path.toLowerCase() && !path.includes("\\"))).toBe(true);
    expect(treeOf(join(root, "nothing"))).toEqual({});
  });

  it("seeds three browser-profile files under data/native-browser, which the policy holds byte for byte", () => {
    const root = temp();
    const made = seedBrowserProfiles(root);
    expect(made).toHaveLength(3);
    for (const path of made) {
      expect(existsSync(join(root, ...path.split("/")))).toBe(true);
      expect(DATA_ROOT_POLICY.contentKept.some((pattern) => pattern.test(path.toLowerCase()))).toBe(true);
    }
  });
});

describe("the planted notice", () => {
  it("is a routine, a failed run and a notice that the daemon's own query joins, in a database that stays whole", () => {
    const root = dataRoot();
    const dbFile = join(root, "data", "tjommie.db");
    expect(pendingNotices(dbFile)).toBe(0);
    plantPendingNotice(dbFile);
    expect(pendingNotices(dbFile)).toBe(1);
    const db = new DatabaseSync(dbFile);
    try {
      // scheduler.rs pending_failures(): the notice only counts if it joins its run, its routine and its bot.
      const rows = db.prepare(
        "SELECT n.run_id, r.routine_id, t.bot_id, n.title, n.body FROM routine_failure_notifications n JOIN routine_runs r ON r.id = n.run_id JOIN routines t ON t.id = r.routine_id ORDER BY n.rowid",
      ).all() as { run_id: string; routine_id: string; bot_id: string; title: string }[];
      expect(rows).toHaveLength(1);
      expect(rows[0]).toMatchObject({ run_id: "acceptance-run", routine_id: "acceptance-routine" });
      expect(db.prepare("PRAGMA foreign_key_check").all()).toEqual([]);
      expect((db.prepare("PRAGMA integrity_check").get() as { integrity_check: string }).integrity_check).toBe("ok");
      // Disabled, so the scheduler never runs it.
      expect((db.prepare("SELECT enabled FROM routines WHERE id = 'acceptance-routine'").get() as { enabled: number }).enabled).toBe(0);
    } finally {
      db.close();
    }
  });

  it("adds exactly three rows, which a baseline taken afterwards holds, and the notice alone may then disappear", () => {
    const root = dataRoot();
    const before = snapshotRoot(root);
    plantPendingNotice(join(root, "data", "tjommie.db"));
    const planted = snapshotRoot(root);
    expect(planted.counts.tables.routines).toBe((before.counts.tables.routines ?? 0) + 1);
    expect(planted.counts.tables.routine_runs).toBe((before.counts.tables.routine_runs ?? 0) + 1);
    expect(planted.counts.tables.routine_failure_notifications).toBe((before.counts.tables.routine_failure_notifications ?? 0) + 1);
    // The new app acknowledges the notice: the daemon deletes the row.
    const db = new DatabaseSync(join(root, "data", "tjommie.db"));
    try { db.prepare("DELETE FROM routine_failure_notifications WHERE run_id = 'acceptance-run'").run(); } finally { db.close(); }
    const acknowledged = snapshotRoot(root);
    expect(compareCounts("x", planted.counts, acknowledged.counts, ["routine_failure_notifications"])).toEqual([]);
    expect(compareCounts("x", planted.counts, acknowledged.counts)).toHaveLength(1);
    // And the files are still all there, with only the database changed.
    expect(compareTrees(planted.tree, acknowledged.tree).problems).toEqual([]);
  });
});

describe("the local update feed", () => {
  it("serves latest.yml and the installer by exact name, with ranges, from 127.0.0.1, and nothing else", async () => {
    const folder = temp();
    writeFileSync(join(folder, "latest.yml"), "version: 0.6.0\n");
    writeFileSync(join(folder, "Tjommie-Setup-0.6.0.exe"), Buffer.from("0123456789"));
    const { server, url } = await startFeed(folder);
    try {
      expect(url).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
      const yml = await fetch(`${url}/latest.yml`);
      expect(yml.status).toBe(200);
      expect(await yml.text()).toBe("version: 0.6.0\n");
      const part = await fetch(`${url}/Tjommie-Setup-0.6.0.exe`, { headers: { range: "bytes=2-5" } });
      expect(part.status).toBe(206);
      expect(part.headers.get("content-range")).toBe("bytes 2-5/10");
      expect(await part.text()).toBe("2345");
      expect((await fetch(`${url}/Tjommie-Setup-0.6.0.exe`, { headers: { range: "bytes=20-30" } })).status).toBe(416);
      // (fetch itself folds a "%2e%2e/" segment away before it sends anything, so only the encoded slash can reach the server.)
      for (const name of ["other.exe", "..%2Fpackage.json", "latest.yml%00", "Latest.yml"]) expect((await fetch(`${url}/${name}`)).status, name).toBe(404);
    } finally {
      server.closeAllConnections();
      await new Promise<void>((done) => server.close(() => done()));
    }
  });
});

describe("the update-test R1's environment, which the new app inherits", () => {
  const install = "C:\\Temp\\tjommie-updatetest\\Tjommie Update Test";
  const env = r1UpdateEnv({ PATH: "p", TJOMMIE_DAEMON_EXE: "C:\\somewhere\\else.exe" }, install, "C:\\Temp\\data", "C:\\Temp\\electron", "http://127.0.0.1:1234");

  it("gives the new app the bundled daemon's path, because the update-test identity starts only a daemon it is handed", () => {
    // Run 2: live.rs takes the bundled daemon for the production identity alone, so the update-test app had no daemon until it was told where one is.
    expect(env.TJOMMIE_DAEMON_EXE).toBe(join(install, "resources", "daemon", "tjommie-daemon.exe"));
    // It replaces what the runner had, rather than being overridden by it.
    expect(env.PATH).toBe("p");
  });

  it("sets the disposable data root, a port the system picks, the feed, and the folders R1's test identity keeps its own state in", () => {
    expect(env).toMatchObject({
      TJOMMIE_HOME: "C:\\Temp\\data", TJOMMIE_USER_DATA: "C:\\Temp\\electron", TJOMMIE_PORT: "0", TJOMMIE_UPDATE_FEED: "http://127.0.0.1:1234", TJOMMIE_BROWSER_HEADLESS: "1",
    });
    expect(env.TJOMMIE_ACCOUNTS_DIR).toBe(join("C:\\Temp\\data", "accounts"));
  });
});

describe("the waiting helpers", () => {
  it("withTimeout gives the result, or the message after the time, and leaves no timer to reject later", async () => {
    await expect(withTimeout(Promise.resolve(7), 1_000, "late")).resolves.toBe(7);
    await expect(withTimeout(new Promise<number>(() => {}), 30, "it took too long")).rejects.toThrow("it took too long");
    await expect(withTimeout(Promise.reject(new Error("its own error")), 1_000, "late")).rejects.toThrow("its own error");
  });

  it("eventually returns the first value that is not undefined or false, and says what it waited for when it gives up", async () => {
    let calls = 0;
    await expect(eventually("a number", 5_000, () => (++calls < 3 ? undefined : calls), 10)).resolves.toBe(3);
    await expect(eventually("the moon", 60, () => false, 10)).rejects.toThrow(/waiting for the moon/);
  });
});

describe("the command line", () => {
  // A child process with no runner in its environment, so what stops it is the guard, on any machine, this one or a hosted runner.
  const run = (args: string[]): { status: number | null; output: string } => {
    const env = { ...process.env };
    for (const key of ["GITHUB_ACTIONS", "RUNNER_ENVIRONMENT", "RUNNER_TEMP", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"]) delete env[key];
    const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8", env, windowsHide: true, cwd: temp() });
    return { status: result.status, output: `${result.stdout}${result.stderr}` };
  };

  it("refuses the commands that change the machine anywhere but a GitHub-hosted runner, before touching anything", () => {
    const root = join(temp(), "root");
    for (const args of [["seed", "--root", root, "--install", root], ["r1-update", "--install", root, "--feed", root, "--root", root, "--user-data", root, "--baseline", join(root, "b.json"), "--expect-version", "0.6.0"], ["r1-export", "--install", root, "--fixture", join(root, "f.json")]]) {
      const result = run(args);
      expect(result.status, args[0]).toBe(1);
      expect(result.output, args[0]).toContain("GitHub-hosted runner");
      expect(existsSync(root), `${args[0]} wrote nothing`).toBe(false);
    }
  });

  it("says how it is used, and which option is missing", () => {
    expect(run(["nope"]).output).toContain("Usage: phase3-acceptance.ts");
    const missing = run(["verify", "--root", temp()]);
    expect(missing.status).toBe(1);
    expect(missing.output).toContain("Missing --baseline");
  });

  it("verifies a root against a baseline end to end, and says what is wrong when it is not as it was", () => {
    const root = dataRoot();
    const baseline = join(temp(), "baseline.json");
    expect(run(["snapshot", "--root", root, "--out", baseline]).status).toBe(0);
    const baselineJson = JSON.parse(readFileSync(baseline, "utf8")) as { counts: { userVersion: number }; tree: Record<string, string> };
    expect(baselineJson.counts.userVersion).toBe(12);
    const report = join(temp(), "report.json");
    const same = run(["verify", "--root", root, "--baseline", baseline, "--report", report]);
    expect(same.status, same.output).toBe(0);
    expect(JSON.parse(readFileSync(report, "utf8")) as { problems: string[] }).toMatchObject({ problems: [], userVersion: { before: 12, after: 12 } });
    // A bot's file is gone and a profile file has changed: both are named, and the run fails.
    rmSync(join(root, "bots", "legacy-bot", "notes.md"));
    writeFileSync(join(root, "data", "native-browser", "acceptance-bot", "Default", "Cookies"), "not the same");
    const broken = run(["verify", "--root", root, "--baseline", baseline]);
    expect(broken.status).toBe(1);
    expect(broken.output).toContain("missing: bots/legacy-bot/notes.md");
    expect(broken.output).toContain("changed: data/native-browser/acceptance-bot/default/cookies");
  });
});
