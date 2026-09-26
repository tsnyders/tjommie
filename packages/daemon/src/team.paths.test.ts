import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join, parse, relative, resolve, sep } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createBot, type StoredBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { fileInside, Team } from "./team.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let root: string;
let bot: StoredBot;
let team: Team;
let otherDriveFolders: string[];

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "t.db"));
  bot = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "Write summaries.", provider: "codex", model: "gpt-5.6-sol" });
  team = new Team({ db, broadcast: () => {}, deliver: () => {}, currentWork: () => null, stateOf: () => "idle" });
  otherDriveFolders = [];
});

afterEach(() => {
  db.close();
  rmSync(root, { recursive: true, force: true });
  for (const folder of otherDriveFolders) rmSync(folder, { recursive: true, force: true });
});

function expectOutsideFile(path: string): void {
  const absolute = resolve(bot.homeFolder, path);
  expect(existsSync(absolute)).toBe(true);
  expect(statSync(absolute).isFile()).toBe(true);
  const rejection = "You can only share files from your own folders.";
  expect(() => fileInside([bot.homeFolder], absolute)).toThrow(new Error(rejection));
  const tool = team.tools().find((entry) => entry.name === "share_file")!;
  expect(() => tool.run(bot.id, tool.input.parse({ path }))).toThrow(new Error(rejection));
}

// Prefer a disposable fixture. Read-only drives can use a root-level existing file;
// the containment test only stats the file, and never opens its contents.
function otherDriveFile(): string | null {
  const ownDrive = parse(root).root.toLowerCase();
  for (const letter of "ABCDEFGHIJKLMNOPQRSTUVWXYZ") {
    const drive = `${letter}:\\`;
    if (drive.toLowerCase() === ownDrive || !existsSync(drive)) continue;
    try {
      const folder = mkdtempSync(join(drive, "tjommie-team-path-test-"));
      otherDriveFolders.push(folder);
      const file = join(folder, "outside.txt");
      writeFileSync(file, "outside the bot's folders");
      return file;
    } catch {
      try {
        const file = readdirSync(drive, { withFileTypes: true }).find((entry) => entry.isFile());
        if (file) return join(drive, file.name);
      } catch {
        // An unavailable or inaccessible drive cannot provide an existing fixture.
      }
    }
  }
  return null;
}

describe("Shared file containment with existing outside files", () => {
  it("rejects an existing sibling file reached through parent traversal", () => {
    const outside = join(dirname(bot.homeFolder), "private");
    mkdirSync(outside);
    const file = join(outside, "secret.txt");
    writeFileSync(file, "private contents");
    const path = relative(bot.homeFolder, file);
    expect(path.startsWith(`..${sep}`)).toBe(true);
    expectOutsideFile(path);
  });

  it("rejects an existing file on another drive", ({ skip }) => {
    if (process.platform !== "win32") return skip("Drive-letter containment requires Windows.");
    const file = otherDriveFile();
    if (!file) return skip("No accessible additional drive provides an existing test file.");
    expect(parse(file).root.toLowerCase()).not.toBe(parse(bot.homeFolder).root.toLowerCase());
    expectOutsideFile(file);
  });

  it("rejects an existing file reached through an inside junction or symlink to an outside folder", ({ skip }) => {
    const outside = join(root, "private");
    mkdirSync(outside);
    writeFileSync(join(outside, "secret.txt"), "private contents");
    const link = join(bot.homeFolder, "outside-link");
    try {
      symlinkSync(outside, link, process.platform === "win32" ? "junction" : "dir");
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      return skip(`The operating system refused to create the directory link: ${reason}`);
    }
    expectOutsideFile(join("outside-link", "secret.txt"));
  });
});
