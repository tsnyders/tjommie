import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";
import { botPrompt, createBot, deleteBot, getStoredBot, listStoredBots, setPaused, updateBot } from "./bots.ts";
import { openDb, type Db } from "./db.ts";
import { RpcError } from "./server.ts";
import { tempRoot } from "./testing.ts";

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

  it("builds a prompt with the name, home folder and instructions", () => {
    const bot = createBot(db, botsDir, input);
    const prompt = botPrompt(bot);
    expect(prompt).toContain("You are Researcher");
    expect(prompt).toContain(bot.homeFolder);
    expect(prompt).toContain("Find sources.");
  });
});
