import { existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { Bot, BotInput, BotPatch } from "@tjommie/protocol";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

export type StoredBot = Omit<Bot, "state">;

type BotRow = {
  id: string;
  name: string;
  color: Bot["color"];
  shape: Bot["shape"];
  instructions: string;
  provider: Bot["provider"];
  model: string;
  autonomy: Bot["autonomy"];
  home_folder: string;
  project_folder: string | null;
  browser: number;
  paused: number;
  limited_until: string | null;
  created_at: string;
};

export function getStoredBot(db: Db, id: string): StoredBot {
  const row = db.prepare("SELECT * FROM bots WHERE id = ?").get(id) as BotRow | undefined;
  if (!row) throw new RpcError("not_found", "No such bot");
  const links = db.prepare("SELECT target_bot_id FROM bot_links WHERE bot_id = ? ORDER BY target_bot_id").all(id) as { target_bot_id: string }[];
  return {
    id: row.id,
    name: row.name,
    color: row.color,
    shape: row.shape,
    instructions: row.instructions,
    provider: row.provider,
    model: row.model,
    autonomy: row.autonomy,
    homeFolder: row.home_folder,
    projectFolder: row.project_folder,
    browser: row.browser === 1,
    paused: row.paused === 1,
    canMessage: links.map((link) => link.target_bot_id),
    limitedUntil: row.limited_until,
    createdAt: row.created_at,
  };
}

export function listStoredBots(db: Db): StoredBot[] {
  const rows = db.prepare("SELECT id FROM bots ORDER BY created_at, rowid").all() as { id: string }[];
  return rows.map((row) => getStoredBot(db, row.id));
}

export function createBot(db: Db, botsDir: string, input: BotInput): StoredBot {
  assertNameFree(db, input.name, null);
  const projectFolder = checkProjectFolder(input.projectFolder ?? null);
  const id = newId();
  const home = join(botsDir, id);
  mkdirSync(join(home, "inbox"), { recursive: true });
  writeFileSync(join(home, "notes.md"), `# ${input.name}'s notes\n`);
  try {
    transaction(db, () => {
      db.prepare(
        `INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, project_folder, browser, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run(id, input.name, input.color, input.shape, input.instructions, input.provider, input.model, input.autonomy ?? "folder", home, projectFolder, input.browser ? 1 : 0, nowIso());
      db.prepare("INSERT INTO conversations (id, kind, bot_id) VALUES (?, 'bot', ?)").run(id, id);
      setLinks(db, id, input.canMessage ?? []);
    });
  } catch (error) {
    rmSync(home, { recursive: true, force: true });
    throw error;
  }
  return getStoredBot(db, id);
}

export function updateBot(db: Db, id: string, patch: BotPatch): StoredBot {
  const bot = getStoredBot(db, id);
  if (patch.provider !== undefined && patch.provider !== bot.provider && patch.model === undefined) {
    throw new RpcError("bad_params", "A model is required when changing provider");
  }
  if (patch.name !== undefined) assertNameFree(db, patch.name, id);
  const next = { ...bot, ...patch, projectFolder: patch.projectFolder === undefined ? bot.projectFolder : checkProjectFolder(patch.projectFolder) };
  transaction(db, () => {
    db.prepare("UPDATE bots SET name = ?, color = ?, shape = ?, instructions = ?, provider = ?, model = ?, autonomy = ?, project_folder = ?, browser = ? WHERE id = ?").run(
      next.name,
      next.color,
      next.shape,
      next.instructions,
      next.provider,
      next.model,
      next.autonomy,
      next.projectFolder,
      next.browser ? 1 : 0,
      id,
    );
    if (patch.canMessage) setLinks(db, id, patch.canMessage);
    // Saved "Always allow" rules are in one CLI's own format, so they don't carry over to the other CLI.
    if (next.provider !== bot.provider) db.prepare("DELETE FROM allow_rules WHERE bot_id = ?").run(id);
  });
  return getStoredBot(db, id);
}

// Deletes the bot, its chat and its home folder. An attached project folder is never touched.
export function deleteBot(db: Db, id: string): void {
  const bot = getStoredBot(db, id);
  db.prepare("DELETE FROM bots WHERE id = ?").run(id);
  rmSync(bot.homeFolder, { recursive: true, force: true });
}

export function setPaused(db: Db, id: string, paused: boolean): void {
  getStoredBot(db, id);
  db.prepare("UPDATE bots SET paused = ? WHERE id = ?").run(paused ? 1 : 0, id);
}

export function setLimitedUntil(db: Db, id: string, until: string | null): void {
  db.prepare("UPDATE bots SET limited_until = ? WHERE id = ?").run(until, id);
}

// The bot's standing instructions, appended to the CLI's own system prompt.
export function botPrompt(bot: StoredBot): string {
  return [
    `You are ${bot.name}, one of the bots a person runs in Tjommie on their own computer.`,
    `Your home folder is ${bot.homeFolder}.${bot.projectFolder ? ` You work in the project folder ${bot.projectFolder}.` : ""}`,
    "Keep notes.md in your home folder up to date with the person's preferences, important facts and short summaries of finished work, and read it when a new session starts.",
    "Files the person attaches are copied into the inbox folder inside your home folder.",
    "Reply in plain, friendly language, and keep replies short unless the person asks for detail.",
    "",
    "What the person wants from you:",
    bot.instructions.trim() || "No instructions yet. Ask what they need.",
  ].join("\n");
}

function assertNameFree(db: Db, name: string, exceptId: string | null): void {
  // ponytail: SQLite lower() folds ASCII only, so "Ñame" and "ñame" count as different names.
  if (db.prepare("SELECT id FROM bots WHERE lower(name) = lower(?) AND id IS NOT ?").get(name, exceptId)) {
    throw new RpcError("name_taken", `There's already a bot called ${name}`);
  }
}

function checkProjectFolder(folder: string | null): string | null {
  if (folder === null) return null;
  if (!isAbsolute(folder) || !existsSync(folder) || !statSync(folder).isDirectory()) {
    throw new RpcError("bad_params", "The project folder must be an existing folder");
  }
  return folder;
}

function setLinks(db: Db, botId: string, targets: string[]): void {
  db.prepare("DELETE FROM bot_links WHERE bot_id = ?").run(botId);
  for (const target of new Set(targets)) {
    if (target === botId) throw new RpcError("bad_params", "A bot can't message itself");
    if (!db.prepare("SELECT 1 FROM bots WHERE id = ?").get(target)) throw new RpcError("not_found", "Can message: no such bot");
    db.prepare("INSERT INTO bot_links (bot_id, target_bot_id) VALUES (?, ?)").run(botId, target);
  }
}
