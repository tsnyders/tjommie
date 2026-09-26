import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { botPrompt, createBot, deleteBot, getStoredBot, listStoredBots, setPaused, sweepDeletedBotFolders, updateBot } from "./bots.ts";
import { newId, openDb, type Db } from "./db.ts";
import { RpcError } from "./server.ts";
import { tempRoot } from "./testing.ts";

vi.mock("node:fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs")>();
  return { ...actual, rmSync: vi.fn(actual.rmSync), renameSync: vi.fn(actual.renameSync) };
});

const input = { name: "Researcher", color: "teal", shape: "droplet", instructions: "Find sources.", provider: "claude", model: "sonnet" } as const;
let db: Db;
let root: string;
let botsDir: string;

beforeEach(() => {
  root = tempRoot();
  botsDir = join(root, "bots");
  db = openDb(join(root, "t.db"));
});

describe("bots store", () => {
  it("creates a bot with defaults, a home folder, notes.md and an inbox", () => {
    const bot = createBot(db, botsDir, input);
    expect(bot).toMatchObject({ name: "Researcher", autonomy: "folder", browser: false, paused: false, projectFolder: null, canMessage: [], limitedUntil: null });
    expect(bot.homeFolder).toBe(join(botsDir, bot.id));
    expect(readFileSync(join(bot.homeFolder, "notes.md"), "utf8")).toContain("Researcher");
    expect(existsSync(join(bot.homeFolder, "inbox"))).toBe(true);
  });

  it("keeps names unique regardless of case", () => {
    createBot(db, botsDir, input);
    expect(() => createBot(db, botsDir, { ...input, name: "researcher" })).toThrow(/already a bot/);
  });

  it("accepts only existing absolute project folders, spaces and accents included", () => {
    const project = join(root, "My Project ñ");
    mkdirSync(project);
    expect(createBot(db, botsDir, { ...input, projectFolder: project }).projectFolder).toBe(project);
    expect(() => createBot(db, botsDir, { ...input, name: "B", projectFolder: join(root, "missing") })).toThrow(/existing folder/);
    expect(() => createBot(db, botsDir, { ...input, name: "C", projectFolder: "relative" })).toThrow(/existing folder/);
  });

  it("validates the Can message list", () => {
    const researcher = createBot(db, botsDir, input);
    const writer = createBot(db, botsDir, { ...input, name: "Writer", canMessage: [researcher.id] });
    expect(writer.canMessage).toEqual([researcher.id]);
    expect(() => updateBot(db, researcher.id, { canMessage: [researcher.id] })).toThrow(/itself/);
    expect(() => updateBot(db, researcher.id, { canMessage: ["nope"] })).toThrow(/no such bot/);
  });

  it("changes only the patched fields, and drops saved rules when the provider changes", () => {
    const bot = createBot(db, botsDir, input);
    db.prepare("INSERT INTO allow_rules (bot_id, rule) VALUES (?, 'Write')").run(bot.id);
    expect(updateBot(db, bot.id, { autonomy: "ask" })).toMatchObject({ autonomy: "ask", name: "Researcher", model: "sonnet", instructions: "Find sources." });
    expect(db.prepare("SELECT COUNT(*) AS n FROM allow_rules").get()).toEqual({ n: 1 });
    updateBot(db, bot.id, { provider: "codex", model: "gpt-5.6-sol" });
    expect(db.prepare("SELECT COUNT(*) AS n FROM allow_rules").get()).toEqual({ n: 0 });
  });

  it("rejects changing provider without a model", () => {
    const bot = createBot(db, botsDir, input);
    let caught: unknown;
    try {
      updateBot(db, bot.id, { provider: "codex" });
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(RpcError);
    expect(caught).toMatchObject({ code: "bad_params", message: "A model is required when changing provider" });
  });

  it("pauses, and deletes the bot with its chat and home folder", () => {
    const bot = createBot(db, botsDir, input);
    setPaused(db, bot.id, true);
    expect(getStoredBot(db, bot.id).paused).toBe(true);
    db.prepare("INSERT INTO messages (id, conversation_id, author, kind, body, status, created_at, seq) VALUES ('m', ?, 'you', 'text', 'hi', 'done', 'now', 1)").run(bot.id);
    deleteBot(db, bot.id);
    expect(listStoredBots(db)).toEqual([]);
    expect(existsSync(bot.homeFolder)).toBe(false);
    expect(db.prepare("SELECT COUNT(*) AS n FROM messages").get()).toEqual({ n: 0 });
  });

  it("keeps the bot when its home folder cannot be removed", () => {
    const bot = createBot(db, botsDir, input);
    const notes = readFileSync(join(bot.homeFolder, "notes.md"), "utf8");
    writeFileSync(join(bot.homeFolder, "inbox", "keep.txt"), "Keep this file");
    mkdirSync(join(bot.homeFolder, "chrome-profile"));
    writeFileSync(join(bot.homeFolder, "chrome-profile", "keep.txt"), "Keep this profile");
    vi.mocked(renameSync).mockImplementationOnce(() => {
      throw new Error("locked");
    });
    let caught: unknown;
    try {
      deleteBot(db, bot.id);
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(RpcError);
    expect(caught).toMatchObject({ code: "busy", message: "The bot folder is busy" });
    expect(getStoredBot(db, bot.id)).toEqual(bot);
    expect(existsSync(bot.homeFolder)).toBe(true);
    expect(readFileSync(join(bot.homeFolder, "notes.md"), "utf8")).toBe(notes);
    expect(readFileSync(join(bot.homeFolder, "inbox", "keep.txt"), "utf8")).toBe("Keep this file");
    expect(readFileSync(join(bot.homeFolder, "chrome-profile", "keep.txt"), "utf8")).toBe("Keep this profile");
  });

  it("builds a prompt with the name, home folder and instructions", () => {
    const bot = createBot(db, botsDir, input);
    const prompt = botPrompt(bot);
    expect(prompt).toContain("You are Researcher");
    expect(prompt).toContain(bot.homeFolder);
    expect(prompt).toContain("Find sources.");
  });

  it("tells the bot about Tjommie's tools, and about its browser only when it has one", () => {
    const bot = createBot(db, botsDir, input);
    const prompt = botPrompt(bot);
    for (const name of ["list_bots", "message_bot", "share_file", "add_routine"]) expect(prompt).toContain(name);
    expect(prompt).not.toContain("browser tools");
    expect(botPrompt({ ...bot, browser: true })).toContain("browser tools");
  });

  it("rejects creating a bot with a project at or below the managed bots folder", () => {
    const owner = createBot(db, botsDir, input);
    for (const projectFolder of [botsDir, owner.homeFolder, join(owner.homeFolder, "inbox")]) {
      expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder })).toThrow(/project folder.*managed bots folder/);
    }
    expect(listStoredBots(db)).toEqual([owner]);
  });

  it("rejects updating a bot with a project at or below the managed bots folder without changing it", () => {
    const owner = createBot(db, botsDir, input);
    const other = createBot(db, botsDir, { ...input, name: "Other" });
    for (const projectFolder of [botsDir, owner.homeFolder, join(owner.homeFolder, "inbox"), other.homeFolder]) {
      expect(() => updateBot(db, other.id, { instructions: "Changed", projectFolder })).toThrow(/project folder.*managed bots folder/);
      expect(getStoredBot(db, other.id)).toEqual(other);
    }
  });

  it("rejects junction aliases into the managed bots folder on create and update", () => {
    const owner = createBot(db, botsDir, input);
    const alias = join(root, "bots-alias");
    symlinkSync(botsDir, alias, "junction");
    for (const projectFolder of [alias, join(alias, owner.id), join(alias, owner.id, "inbox")]) {
      expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder })).toThrow(/project folder.*managed bots folder/);
      expect(() => updateBot(db, owner.id, { projectFolder })).toThrow(/project folder.*managed bots folder/);
    }
    expect(getStoredBot(db, owner.id)).toEqual(owner);
  });

  it("checks the real managed bots folder when its configured path is a junction", () => {
    const physical = join(root, "physical-bots");
    mkdirSync(physical);
    symlinkSync(physical, botsDir, "junction");
    const owner = createBot(db, botsDir, input);
    expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder: physical })).toThrow(/project folder.*managed bots folder/);
    expect(() => updateBot(db, owner.id, { projectFolder: join(physical, owner.id) })).toThrow(/project folder.*managed bots folder/);
  });

  it("rejects project paths under the managed bots folder even when a junction leads outside", () => {
    const owner = createBot(db, botsDir, input);
    const project = join(root, "project");
    mkdirSync(project);
    const alias = join(owner.homeFolder, "project-link");
    symlinkSync(project, alias, "junction");
    expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder: alias })).toThrow(/project folder.*managed bots folder/);
    expect(() => updateBot(db, owner.id, { projectFolder: alias })).toThrow(/project folder.*managed bots folder/);
  });

  it("accepts a separate project folder whose name shares the managed bots prefix", () => {
    const project = join(root, "bots-project");
    mkdirSync(project);
    const bot = createBot(db, botsDir, { ...input, projectFolder: project });
    expect(bot.projectFolder).toBe(project);
    expect(updateBot(db, bot.id, { projectFolder: null }).projectFolder).toBeNull();
    expect(updateBot(db, bot.id, { projectFolder: project }).projectFolder).toBe(project);
  });

  it("restores the untouched folder if the database refuses deletion", () => {
    const bot = createBot(db, botsDir, input);
    db.exec("CREATE TRIGGER refuse_bot_delete BEFORE DELETE ON bots BEGIN SELECT RAISE(ABORT, 'keep bot'); END");
    expect(() => deleteBot(db, bot.id)).toThrow(/keep bot/);
    expect(getStoredBot(db, bot.id)).toEqual(bot);
    expect(readFileSync(join(bot.homeFolder, "notes.md"), "utf8")).toContain("Researcher");
  });

  it("does not restore a partially removed folder after successful database deletion", () => {
    const bot = createBot(db, botsDir, input);
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(rmSync).mockImplementationOnce(() => { throw new Error("locked child"); });
    try {
      deleteBot(db, bot.id);
      expect(existsSync(bot.homeFolder)).toBe(false);
      expect(listStoredBots(db)).toEqual([]);
      expect(logged).toHaveBeenCalledWith("Could not finish removing a deleted bot folder.", expect.any(Error));
      const leftover = readdirSync(botsDir).find((name) => name.startsWith(`${bot.id}.deleting-`));
      expect(leftover).toBeDefined();
      sweepDeletedBotFolders(db, botsDir);
      expect(readdirSync(botsDir)).toEqual([]);
    } finally {
      logged.mockRestore();
    }
  });

  it("logs leftover cleanup failures and retries them on the next sweep", () => {
    mkdirSync(botsDir);
    const folders = [join(botsDir, `${newId()}.deleting-${newId()}`), join(botsDir, `${newId()}.deleting-${newId()}`)];
    for (const folder of folders) mkdirSync(folder);
    const error = new Error("Still locked");
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(rmSync).mockImplementationOnce(() => { throw error; });
    try {
      expect(() => sweepDeletedBotFolders(db, botsDir)).not.toThrow();
      expect(folders.filter(existsSync)).toHaveLength(1);
      expect(logged).toHaveBeenCalledWith("Could not finish removing a deleted bot folder.", error);
      sweepDeletedBotFolders(db, botsDir);
      expect(folders.some(existsSync)).toBe(false);
    } finally {
      logged.mockRestore();
    }
  });

  it("preserves live bot folders and unrelated entries during leftover cleanup", () => {
    const bot = createBot(db, botsDir, input);
    const liveRename = `${bot.homeFolder}.deleting-${newId()}`;
    renameSync(bot.homeFolder, liveRename);
    const unrelated = join(botsDir, "notes.deleting-keep");
    mkdirSync(unrelated);
    const ordinary = join(botsDir, "other-folder");
    mkdirSync(ordinary);
    const file = join(botsDir, `${newId()}.deleting-${newId()}`);
    writeFileSync(file, "Keep file");
    sweepDeletedBotFolders(db, botsDir);
    expect(getStoredBot(db, bot.id)).toEqual(bot);
    expect(readFileSync(join(liveRename, "notes.md"), "utf8")).toContain("Researcher");
    expect([unrelated, ordinary, file].every(existsSync)).toBe(true);
  });

  it("keeps a registered live home even when its name looks like another bot's deletion leftover", () => {
    const bot = createBot(db, botsDir, input);
    const home = join(botsDir, `${newId()}.deleting-${newId()}`);
    renameSync(bot.homeFolder, home);
    db.prepare("UPDATE bots SET home_folder = ? WHERE id = ?").run(home, bot.id);
    sweepDeletedBotFolders(db, botsDir);
    expect(readFileSync(join(home, "notes.md"), "utf8")).toContain("Researcher");
  });

  it("never removes junction targets while sweeping a deleted bot's folder", () => {
    mkdirSync(botsDir);
    const outside = join(root, "outside");
    mkdirSync(outside);
    writeFileSync(join(outside, "keep.txt"), "Do not remove");
    const leftover = join(botsDir, `${newId()}.deleting-${newId()}`);
    mkdirSync(leftover);
    symlinkSync(outside, join(leftover, "link"), "junction");
    const linkedEntry = join(botsDir, `${newId()}.deleting-${newId()}`);
    symlinkSync(outside, linkedEntry, "junction");
    sweepDeletedBotFolders(db, botsDir);
    expect(existsSync(leftover)).toBe(false);
    expect(existsSync(linkedEntry)).toBe(true);
    expect(readFileSync(join(outside, "keep.txt"), "utf8")).toBe("Do not remove");
  });

  it("rejects the data root and its ancestors on create and update", () => {
    const bot = createBot(db, botsDir, input);
    for (const projectFolder of [root, dirname(root)]) {
      expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder })).toThrow(/doesn't contain Tjommie's own data folder/);
      expect(() => updateBot(db, bot.id, { projectFolder })).toThrow(/doesn't contain Tjommie's own data folder/);
    }
    expect(getStoredBot(db, bot.id)).toEqual(bot);
  });

  it("rejects junction aliases of the data root and its ancestors", () => {
    const bot = createBot(db, botsDir, input);
    const aliases = tempRoot();
    for (const [name, target] of [["root", root], ["parent", dirname(root)]]) {
      const alias = join(aliases, name);
      symlinkSync(target, alias, "junction");
      expect(() => createBot(db, botsDir, { ...input, name: "Other", projectFolder: alias })).toThrow(/doesn't contain Tjommie's own data folder/);
      expect(() => updateBot(db, bot.id, { projectFolder: alias })).toThrow(/doesn't contain Tjommie's own data folder/);
    }
  });

  it("checks the physical data root when its configured path is a junction", () => {
    const physicalParent = tempRoot();
    const physicalRoot = join(physicalParent, "data");
    mkdirSync(physicalRoot);
    const alias = join(root, "data-link");
    symlinkSync(physicalRoot, alias, "junction");
    const aliasedBots = join(alias, "bots");
    const bot = createBot(db, aliasedBots, input);
    for (const projectFolder of [physicalRoot, physicalParent, alias, root]) {
      expect(() => createBot(db, aliasedBots, { ...input, name: "Other", projectFolder })).toThrow(/doesn't contain Tjommie's own data folder/);
      expect(() => updateBot(db, bot.id, { projectFolder })).toThrow(/doesn't contain Tjommie's own data folder/);
    }
  });
});
