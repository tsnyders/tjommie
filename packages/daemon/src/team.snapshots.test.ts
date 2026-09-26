import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBot, deleteBot, type StoredBot } from "./bots.ts";
import { getMessage, history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, newChain, type WorkItem } from "./queue.ts";
import { Team } from "./team.ts";
import { tempRoot } from "./testing.ts";

vi.mock("node:fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs")>();
  return { ...actual, renameSync: vi.fn(actual.renameSync) };
});

let root: string;
let db: Db;
let team: Team;
let writer: StoredBot;
let asker: StoredBot;
let current: Map<string, WorkItem>;
let delivered: WorkItem[];
const done = { outcome: "done" as const, text: "Here is the file.", textMessageId: null, error: null };

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "team.db"));
  writer = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "gpt-5.6-sol" });
  asker = createBot(db, join(root, "bots"), { name: "Asker", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet", canMessage: [writer.id] });
  current = new Map();
  delivered = [];
  team = new Team({ db, broadcast: () => {}, deliver: (work) => { delivered.push(work); }, currentWork: (id) => current.get(id) ?? null, stateOf: () => "idle" });
});

afterEach(() => {
  vi.restoreAllMocks();
  db.close();
  rmSync(root, { recursive: true, force: true });
});

const call = (botId: string, name: string, args: unknown) => {
  const tool = team.tools().find((entry) => entry.name === name)!;
  return tool.run(botId, tool.input.parse(args));
};
const ask = () => {
  current.set(asker.id, enqueue(db, { botId: asker.id, source: "you", text: "Help", chainId: newChain(db) }));
  call(asker.id, "message_bot", { bot: "Writer", message: "Share a file." });
  const request = delivered.at(-1)!;
  current.set(writer.id, request);
  return request;
};
const share = (conversationId: string, name = "answer.txt") => {
  writeFileSync(join(writer.homeFolder, name), "snapshot contents");
  call(writer.id, "share_file", { path: name });
  return history(db, conversationId, undefined, 1)[0];
};

describe("Team snapshot lifecycle", () => {
  it("delivers and releases a snapshot shared while retrying a failed exchange", () => {
    const request = ask();
    team.turnEnded(request, { ...done, outcome: "failed", error: "Try again" });
    const retry = enqueue(db, { botId: writer.id, source: "bot_request", text: request.text, conversationId: request.conversationId, exchangeId: request.exchangeId, chainId: newChain(db) });
    current.set(writer.id, retry);
    const card = share(retry.conversationId);
    const stage = dirname(String(card.data?.path));
    team.turnEnded(retry, done);
    const expected = join(asker.homeFolder, "inbox", "answer.txt");
    expect(getMessage(db, card.id).data?.path).toBe(expected);
    expect(readFileSync(expected, "utf8")).toBe("snapshot contents");
    expect(existsSync(stage)).toBe(false);
  });

  it("removes staging after all copies succeed and points cards and room files at the recipient inbox", () => {
    const request = ask();
    const cards = [share(request.conversationId, "one.txt"), share(request.conversationId, "two.txt")];
    const stages = cards.map((card) => String(card.data?.path));
    expect(stages.every(existsSync)).toBe(true);
    team.turnEnded(request, done);
    for (const [index, card] of cards.entries()) {
      const updated = getMessage(db, card.id);
      const expected = join(asker.homeFolder, "inbox", index === 0 ? "one.txt" : "two.txt");
      expect(updated.data?.path).toBe(expected);
      expect(readFileSync(expected, "utf8")).toBe("snapshot contents");
      expect(existsSync(dirname(stages[index]))).toBe(false);
    }
    const roomFiles = history(db, "team", undefined, 1)[0].data?.files;
    expect(roomFiles).toEqual(cards.map((card) => ({ name: card.data?.name, path: join(asker.homeFolder, "inbox", String(card.data?.name)) })));
    expect(readdirSync(join(root, "shared-files"))).toEqual([]);
  });

  it("uses a durable inbox copy and immediately removes staging for a share in the bot chat", () => {
    const card = share(writer.id);
    const expected = join(writer.homeFolder, "inbox", "answer.txt");
    expect(card.data?.path).toBe(expected);
    rmSync(join(writer.homeFolder, "answer.txt"));
    expect(readFileSync(expected, "utf8")).toBe("snapshot contents");
    expect(readdirSync(join(root, "shared-files"))).toEqual([]);
  });

  it("keeps failed-delivery staging pending until successful bot deletion removes it", () => {
    const request = ask();
    const card = share(request.conversationId);
    const staging = dirname(String(card.data?.path));
    rmSync(join(asker.homeFolder, "inbox"), { recursive: true });
    writeFileSync(join(asker.homeFolder, "inbox"), "blocks copying");
    team.turnEnded(request, done);
    expect(existsSync(staging)).toBe(true);
    const handle = team.botDeleting(writer.id);
    expect(existsSync(staging)).toBe(true);
    deleteBot(db, writer.id);
    team.botDeleted(handle);
    expect(existsSync(staging)).toBe(false);
  });

  it("retains pending snapshots when deletion finalization rolls back", () => {
    const request = ask();
    const card = share(request.conversationId);
    const staging = dirname(String(card.data?.path));
    const handle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    db.exec("CREATE TRIGGER reject_deletion BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' BEGIN SELECT RAISE(ABORT, 'cannot notify'); END");
    expect(() => team.botDeleted(handle)).toThrow("cannot notify");
    expect(existsSync(staging)).toBe(true);
    db.exec("DROP TRIGGER reject_deletion");
    team.botDeleted(handle);
    expect(existsSync(staging)).toBe(false);
  });

  it("restores moved snapshots and database state when a later cleanup rename fails", () => {
    const request = ask();
    const cards = [share(request.conversationId, "one.txt"), share(request.conversationId, "two.txt")];
    const folders = cards.map((card) => dirname(String(card.data?.path)));
    const handle = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    const before = db.prepare("SELECT * FROM exchanges").all();
    const actualRename = vi.mocked(renameSync).getMockImplementation()!;
    let renames = 0;
    vi.mocked(renameSync).mockImplementation((from, to) => {
      if (++renames === 2) throw new Error("cleanup locked");
      actualRename(from, to);
    });
    const sentBefore = delivered.length;
    expect(() => team.botDeleted(handle)).toThrow("cleanup locked");
    expect(db.prepare("SELECT * FROM exchanges").all()).toEqual(before);
    expect(delivered).toHaveLength(sentBefore);
    expect(folders.every(existsSync)).toBe(true);
    expect(readdirSync(join(root, "shared-files")).every((name) => name.startsWith("share-"))).toBe(true);
    team.botDeleted(handle);
    expect(folders.some(existsSync)).toBe(false);
  });

  it("sweeps only orphan snapshot folders older than 24 hours at startup", () => {
    const staging = join(root, "shared-files");
    const now = Date.now();
    const old = join(staging, "share-orphan");
    const cleanup = join(staging, "cleanup-orphan");
    const recent = join(staging, "share-recent");
    const unrelated = join(staging, "other-folder");
    for (const folder of [old, cleanup, recent, unrelated]) {
      mkdirSync(folder, { recursive: true });
      writeFileSync(join(folder, "file.txt"), "keep or sweep");
    }
    const yesterday = new Date(now - 25 * 60 * 60 * 1000);
    utimesSync(old, yesterday, yesterday);
    utimesSync(cleanup, yesterday, yesterday);
    utimesSync(unrelated, yesterday, yesterday);
    team.start();
    expect(existsSync(old)).toBe(false);
    expect(existsSync(cleanup)).toBe(false);
    expect(existsSync(recent)).toBe(true);
    expect(existsSync(unrelated)).toBe(true);
    expect(() => team.start()).not.toThrow();
  });
});
