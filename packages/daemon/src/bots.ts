import { existsSync, mkdirSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import type { Bot, BotInput, BotPatch } from "@tjommie/protocol";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

export type StoredBot = Omit<Bot, "state" | "setupReason">;

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
  const projectFolder = checkProjectFolder(input.projectFolder ?? null, botsDir);
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
  const next = { ...bot, ...patch, projectFolder: patch.projectFolder === undefined ? bot.projectFolder : checkProjectFolder(patch.projectFolder, dirname(bot.homeFolder)) };
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
  const removed = `${bot.homeFolder}.deleting-${newId()}`;
  const hadFolder = existsSync(bot.homeFolder);
  try {
    // A locked folder must fail before any of its contents are removed.
    if (hadFolder) renameSync(bot.homeFolder, removed);
  } catch {
    throw new RpcError("busy", "The bot folder is busy");
  }
  try {
    db.prepare("DELETE FROM bots WHERE id = ?").run(id);
  } catch (error) {
    if (hadFolder) renameSync(removed, bot.homeFolder);
    throw error;
  }
  if (hadFolder) {
    try {
      rmSync(removed, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    } catch (error) {
      // The bot is deleted. Never restore a folder after a partial recursive removal.
      console.error("Could not finish removing a deleted bot folder.", error);
    }
  }
}

// Retry folders left behind after a committed deletion. A crash before the row was
// deleted can leave the same rename behind, so never remove a live bot's folder.
export function sweepDeletedBotFolders(db: Db, botsDir: string): void {
  if (!existsSync(botsDir)) return;
  try {
    const live = db.prepare("SELECT id, home_folder FROM bots").all() as { id: string; home_folder: string }[];
    const ids = new Set(live.map((bot) => bot.id.toLowerCase()));
    const pathKey = (path: string) => process.platform === "win32" ? resolve(path).toLowerCase() : resolve(path);
    const homes = live.map((bot) => pathKey(bot.home_folder));
    const pattern = /^([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})\.deleting-[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
    for (const entry of readdirSync(botsDir, { withFileTypes: true })) {
      const match = pattern.exec(entry.name);
      // Directory links are not deletion leftovers. rmSync also unlinks nested
      // junctions without following their targets when removing a real folder.
      if (!entry.isDirectory() || !match || ids.has(match[1].toLowerCase())) continue;
      const folder = join(botsDir, entry.name);
      const candidate = pathKey(folder);
      if (homes.some((home) => home === candidate || home.startsWith(`${candidate}${sep}`))) continue;
      try {
        rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
      } catch (error) {
        console.error("Could not finish removing a deleted bot folder.", error);
      }
    }
  } catch (error) {
    console.error("Could not check for deleted bot folders.", error);
  }
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
    "Tjommie's tools: list_bots shows the bots you may ask for help; message_bot sends one a request, and its answer arrives later as a new message, so don't wait for it; share_file shows the person a file; add_routine proposes work on a schedule.",
    ...(bot.browser
      ? ["You have your own browser through the browser tools. Sites the person signed in to there stay signed in. If the browser says it's already in use, ask the person to close your browser window."]
      : []),
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

function checkProjectFolder(folder: string | null, botsDir: string): string | null {
  if (folder === null) return null;
  if (!isAbsolute(folder) || !existsSync(folder) || !statSync(folder).isDirectory()) {
    throw new RpcError("bad_params", "The project folder must be an existing folder");
  }
  const atOrInside = (base: string, target: string): boolean => {
    const path = relative(base, target);
    return path === "" || (path !== ".." && !path.startsWith(`..${sep}`) && !isAbsolute(path));
  };
  // Reject both configured paths and junction aliases, including a managed root that is itself a junction.
  if (atOrInside(resolve(botsDir), resolve(folder)) || (existsSync(botsDir) && atOrInside(realpathSync.native(botsDir), realpathSync.native(folder)))) {
    throw new RpcError("bad_params", "The project folder must be outside the managed bots folder");
  }
  const dataRoot = dirname(botsDir);
  if (atOrInside(resolve(folder), resolve(dataRoot)) || atOrInside(realpathSync.native(folder), realpathSync.native(dataRoot))) {
    throw new RpcError("bad_params", "Pick a folder that doesn't contain Tjommie's own data folder.");
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
