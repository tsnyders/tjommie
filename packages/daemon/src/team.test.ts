import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, truncateSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { TEAM_ROOM_ID } from "@tjommie/protocol";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, updateBot, type StoredBot } from "./bots.ts";
import { addMessage, history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, newChain, setStatus, type WorkItem } from "./queue.ts";
import { listRoutines } from "./routines.ts";
import { setSettings } from "./settings.ts";
import { Team, fileInside, findMentions, getExchange } from "./team.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
let root: string;
let team: Team;
let delivered: WorkItem[];
let current: Map<string, WorkItem>;
let events: { event: string; data: unknown }[];
let researcher: StoredBot;
let writer: StoredBot;

beforeEach(() => {
  root = tempRoot();
  db = openDb(join(root, "t.db"));
  const bots = join(root, "bots");
  writer = createBot(db, bots, { name: "Writer", color: "blue", shape: "circle", instructions: "Writes one-page summaries.\nMore detail here.", provider: "codex", model: "gpt-5.6-sol" });
  researcher = createBot(db, bots, { name: "Researcher", color: "teal", shape: "droplet", instructions: "Finds sources.", provider: "claude", model: "sonnet", canMessage: [writer.id] });
  writer = updateBot(db, writer.id, { canMessage: [researcher.id] });
  delivered = [];
  current = new Map();
  events = [];
  team = new Team({
    db,
    broadcast: (event, data) => events.push({ event, data }),
    deliver: (work) => delivered.push(work),
    stopItem: () => {},
    currentWork: (botId) => current.get(botId) ?? null,
    stateOf: () => "idle",
  });
});

afterEach(() => {
  db.close();
  rmSync(root, { recursive: true, force: true });
});

const tool = (name: string) => team.tools().find((t) => t.name === name)!;
const call = (botId: string, name: string, args: unknown) => tool(name).run(botId, tool(name).input.parse(args));
// A turn started by the person in the bot's own chat, with a chain of its own.
const chatTurn = (botId: string) => {
  const item = enqueue(db, { botId, source: "you", text: "hi", chainId: newChain(db) });
  current.set(botId, item);
  return item;
};
const done = (text: string | null, textMessageId: string | null = null) => ({ outcome: "done" as const, text, textMessageId, error: null });

describe("Team tools", () => {
  it("lists only the bots on the caller's Can message list", () => {
    expect(call(researcher.id, "list_bots", {})).toBe("- Writer (idle): Writes one-page summaries.");
    const loner = createBot(db, join(root, "bots"), { name: "Loner", color: "gray", shape: "pill", instructions: "", provider: "claude", model: "haiku" });
    expect(call(loner.id, "list_bots", {})).toMatch(/can't message any bots yet/);
  });

  it("delivers a request, shows it in both chats and the room, and sends the answer and files back", () => {
    chatTurn(researcher.id);
    expect(call(researcher.id, "message_bot", { bot: "@writer", message: "Turn these notes into a summary." })).toBe("Sent. Writer will reply.");
    const [request] = delivered;
    const exchangeId = request.exchangeId!;
    expect(request).toMatchObject({ botId: writer.id, source: "bot_request", conversationId: exchangeId, chainId: current.get(researcher.id)!.chainId });
    expect(request.text).toContain("Message from Researcher");
    for (const botId of [researcher.id, writer.id]) {
      expect(history(db, botId, undefined, 5).at(-1)).toMatchObject({ kind: "handoff", body: "Turn these notes into a summary.", data: { exchangeId, messages: 1, status: "waiting" } });
    }
    expect(history(db, TEAM_ROOM_ID, undefined, 5)).toMatchObject([{ author: researcher.id, body: "Turn these notes into a summary.", data: { exchangeId, toBotId: writer.id } }]);

    // Writer's turn: it shares a file, then answers. The runner writes the answer into the exchange.
    current.set(writer.id, request);
    writeFileSync(join(writer.homeFolder, "summary.md"), "# Summary");
    expect(call(writer.id, "share_file", { path: "summary.md" })).toBe("Shared summary.md.");
    addMessage(db, { conversationId: exchangeId, author: writer.id, kind: "text", body: "Here's the summary.", status: "done" });
    team.turnEnded(request, done("Here's the summary."));

    expect(getExchange(db, exchangeId)).toMatchObject({ status: "replied", reply: "Here's the summary." });
    const reply = delivered[1];
    expect(reply).toMatchObject({ botId: researcher.id, source: "bot_reply", conversationId: researcher.id, exchangeId });
    expect(reply.text).toContain("Writer replied");
    expect(reply.text).toContain("Here's the summary.");
    expect(reply.text).toContain("summary.md");
    expect(readFileSync(join(researcher.homeFolder, "inbox", "summary.md"), "utf8")).toBe("# Summary");
    expect(history(db, researcher.id, undefined, 5).find((m) => m.kind === "handoff")?.data).toMatchObject({ messages: 2, status: "replied" });
    expect(history(db, TEAM_ROOM_ID, undefined, 5).at(-1)).toMatchObject({ author: writer.id, body: "Here's the summary.", data: { toBotId: researcher.id, files: [{ name: "summary.md" }] } });
  });

  it("refuses unknown bots, bots off the list, and the caller itself", () => {
    createBot(db, join(root, "bots"), { name: "Loner", color: "gray", shape: "pill", instructions: "", provider: "claude", model: "haiku" });
    expect(() => call(researcher.id, "message_bot", { bot: "Nobody", message: "hi" })).toThrow(/no bot called Nobody/);
    expect(() => call(researcher.id, "message_bot", { bot: "Loner", message: "hi" })).toThrow(/not allowed to message Loner/);
    expect(() => call(researcher.id, "message_bot", { bot: "Researcher", message: "hi" })).toThrow(/can't message yourself/);
    expect(delivered).toEqual([]);
  });

  it("stops two bots asking each other in a loop at the chain limit (chain limit)", () => {
    setSettings(db, { chainLimit: 2 });
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Ask me something back." });
    current.set(writer.id, delivered[0]);
    call(writer.id, "message_bot", { bot: "Researcher", message: "What do you need?" });
    expect(delivered[1].chainId).toBe(delivered[0].chainId);
    expect(() => call(researcher.id, "message_bot", { bot: "Writer", message: "Again." })).toThrow(/^Limit reached, ask the user/);
    chatTurn(researcher.id); // the person's next message starts a new chain
    expect(call(researcher.id, "message_bot", { bot: "Writer", message: "Fresh start." })).toBe("Sent. Writer will reply.");
  });

  it("tells the asking bot when the other bot couldn't answer", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Summarize." });
    team.turnEnded(delivered[0], { outcome: "failed", text: null, textMessageId: null, error: "Codex exited with code 1" });
    expect(delivered[1]).toMatchObject({ botId: researcher.id, source: "bot_reply" });
    expect(delivered[1].text).toContain("Writer couldn't finish your request");
    expect(delivered[1].text).toContain("Codex exited with code 1");
    expect(getExchange(db, delivered[0].exchangeId!).status).toBe("failed");
  });

  it("ends a deleted asker's exchanges without errors (requester deleted)", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "First." });
    call(researcher.id, "message_bot", { bot: "Writer", message: "Second." });
    const [started, waiting] = delivered;
    setStatus(db, started.id, "running");
    const deleting = team.botDeleting(researcher.id);
    deleteBot(db, researcher.id);
    team.botDeleted(deleting);
    expect(db.prepare("SELECT status FROM work_items WHERE id = ?").get(waiting.id)).toEqual({ status: "failed" });
    expect(getExchange(db, waiting.exchangeId!).status).toBe("failed");
    current.set(writer.id, started);
    expect(() => team.turnEnded(started, done("Done anyway."))).not.toThrow();
    expect(delivered).toHaveLength(2); // nothing goes back to the deleted bot
    expect(getExchange(db, started.exchangeId!).status).toBe("failed");
  });

  it("tells the asking bot when the bot it asked is deleted", () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Summarize." });
    const deleting = team.botDeleting(writer.id);
    deleteBot(db, writer.id);
    team.botDeleted(deleting);
    expect(delivered[1]).toMatchObject({ botId: researcher.id, source: "bot_reply" });
    expect(delivered[1].text).toContain("Writer was deleted");
    expect(getExchange(db, delivered[0].exchangeId!).status).toBe("failed");
  });

  it("shares only existing files inside the bot's own folders (share outside folder)", () => {
    const outside = join(root, "outside");
    mkdirSync(outside);
    writeFileSync(join(outside, "secret.txt"), "no");
    writeFileSync(join(writer.homeFolder, "ok.txt"), "yes");
    symlinkSync(outside, join(writer.homeFolder, "link"), "junction");
    expect(call(writer.id, "share_file", { path: "ok.txt", caption: "The summary" })).toBe("Shared ok.txt.");
    expect(history(db, writer.id, undefined, 1)[0]).toMatchObject({ kind: "file", body: "The summary", data: { name: "ok.txt", size: 3 } });
    for (const path of [join("..", "..", "outside", "secret.txt"), join(outside, "secret.txt"), join("link", "secret.txt")]) {
      expect(() => call(writer.id, "share_file", { path })).toThrow(/only share files from your own folders/);
    }
    expect(() => call(writer.id, "share_file", { path: "missing.txt" })).toThrow(/no file/);
  });

  it("proposes a routine as a card, and creates it only when the person approves", () => {
    expect(call(writer.id, "add_routine", { name: "Morning check", days: ["mon", "tue", "wed", "thu", "fri"], times: ["08:48"], prompt: "Check pnet.co.za", time_limit_minutes: 60 })).toMatch(/^Proposed/);
    const first = events.find((e) => e.event === "approval.added")!.data as import("@tjommie/protocol").Approval;
    expect(first).toMatchObject({ kind: "routine", summary: "Add a routine: Morning check, weekdays at 08:48, stop after 1 hour." });
    expect(listRoutines(db, writer.id)).toEqual([]);
    team.answerRoutineProposal(first.id, "once");
    expect(listRoutines(db, writer.id)).toMatchObject([{ name: "Morning check", days: [1, 2, 3, 4, 5], times: ["08:48"], timeLimitMinutes: 60 }]);
    call(writer.id, "add_routine", { name: "Other", days: ["sat"], times: ["10:00"], prompt: "Tidy the inbox" });
    const second = events.filter((e) => e.event === "approval.added")[1].data as import("@tjommie/protocol").Approval;
    team.answerRoutineProposal(second.id, "deny");
    expect(listRoutines(db, writer.id)).toHaveLength(1);
    expect(getApproval(db, second.id).status).toBe("denied");
  });
});

describe("Team room", () => {
  it("asks each bot a post mentions, with the recent room messages, on one chain", () => {
    team.postToRoom("Morning all.");
    expect(delivered).toEqual([]);
    const post = team.postToRoom("@Writer and @researcher, what's new?");
    expect(delivered.map((item) => item.botId).sort()).toEqual([writer.id, researcher.id].sort());
    expect(delivered[0]).toMatchObject({ source: "room", conversationId: TEAM_ROOM_ID });
    expect(delivered.map((item) => item.messageId)).toEqual([post.id, post.id]);
    expect(delivered[0].chainId).toBe(delivered[1].chainId);
    expect(delivered[0].text).toContain("Person: Morning all.");
    expect(delivered[0].text).toContain("what's new?");
  });

  it("notes a room answer in the bot's own chat", () => {
    team.postToRoom("@Writer hi");
    team.turnEnded(delivered[0], done("Hello!", "m1"));
    expect(history(db, writer.id, undefined, 1)[0]).toMatchObject({ kind: "handoff", body: "Answered in the Team room", data: { roomMessageId: "m1" } });
  });
});

describe("findMentions", () => {
  const bots = [
    { id: "w", name: "Writer" },
    { id: "cv", name: "CV Writer" },
    { id: "b", name: "Bot" },
    { id: "b2", name: "Bot 2" },
  ];

  it("matches whole names, longest first, and ignores email addresses", () => {
    expect(findMentions("@CV Writer can you help?", bots)).toEqual(["cv"]);
    expect(findMentions("@writer and @cv writer!", bots)).toEqual(["w", "cv"]);
    expect(findMentions("@Bot 2 and @Bot 2 again", bots)).toEqual(["b2"]);
    expect(findMentions("mail me@writer.com or @Writers", bots)).toEqual([]);
    expect(findMentions("mail me_@writer.com or me+@writer.com or @Writer_extra", bots)).toEqual([]);
    expect(findMentions("@everyone stop", bots)).toEqual(["w", "cv", "b", "b2"]);
    expect(findMentions("@everyone stop", [...bots, { id: "all", name: "Everyone" }])).toEqual(["w", "cv", "b", "b2", "all"]);
  });
});

describe("Shared file snapshots", () => {
  const request = () => {
    chatTurn(researcher.id);
    call(researcher.id, "message_bot", { bot: "Writer", message: "Send a file." });
    current.set(writer.id, delivered[0]);
    return delivered[0];
  };

  it("delivers the snapshot after a shared source is swapped for an outside junction", () => {
    const item = request();
    const folder = join(writer.homeFolder, "output");
    const outside = join(root, "private");
    mkdirSync(folder);
    mkdirSync(outside);
    writeFileSync(join(folder, "answer.txt"), "safe original");
    writeFileSync(join(outside, "answer.txt"), "private replacement");
    call(writer.id, "share_file", { path: "output/answer.txt" });
    rmSync(folder, { recursive: true });
    symlinkSync(outside, folder, "junction");
    team.turnEnded(item, done("Here it is."));
    expect(readFileSync(join(researcher.homeFolder, "inbox", "answer.txt"), "utf8")).toBe("safe original");
  });

  it("delivers the snapshot after the source is deleted", () => {
    const item = request();
    const source = join(writer.homeFolder, "answer.txt");
    writeFileSync(source, "safe original");
    call(writer.id, "share_file", { path: source });
    rmSync(source);
    expect(() => team.turnEnded(item, done("Here it is."))).not.toThrow();
    expect(readFileSync(join(researcher.homeFolder, "inbox", "answer.txt"), "utf8")).toBe("safe original");
  });

  it("delivers the bounded snapshot after the source grows beyond the share limit", () => {
    const item = request();
    const source = join(writer.homeFolder, "answer.txt");
    writeFileSync(source, "safe original");
    call(writer.id, "share_file", { path: source });
    truncateSync(source, 200 * 1024 * 1024 + 1);
    team.turnEnded(item, done("Here it is."));
    expect(statSync(join(researcher.homeFolder, "inbox", "answer.txt")).size).toBe(13);
    expect(readFileSync(join(researcher.homeFolder, "inbox", "answer.txt"), "utf8")).toBe("safe original");
  });

  it("fails visibly without publishing a reply when an inbox copy fails", () => {
    const item = request();
    writeFileSync(join(writer.homeFolder, "answer.txt"), "safe");
    call(writer.id, "share_file", { path: "answer.txt" });
    rmSync(join(researcher.homeFolder, "inbox"), { recursive: true });
    writeFileSync(join(researcher.homeFolder, "inbox"), "blocks copying");
    events.length = 0;
    expect(() => team.turnEnded(item, done("The answer."))).not.toThrow();
    expect(getExchange(db, item.exchangeId!).status).toBe("failed");
    expect(history(db, TEAM_ROOM_ID, undefined, 10).some((m) => m.body === "The answer.")).toBe(false);
    expect(delivered[1].text).toContain("couldn't finish your request");
    expect(events).not.toContainEqual(expect.objectContaining({ data: expect.objectContaining({ body: "The answer." }) }));
  });

  it("rejects folders and oversized sources", () => {
    expect(() => fileInside([writer.homeFolder], join(writer.homeFolder, "inbox"))).toThrow(/folder/);
    const source = join(writer.homeFolder, "large.bin");
    writeFileSync(source, "");
    truncateSync(source, 200 * 1024 * 1024 + 1);
    expect(() => call(writer.id, "share_file", { path: source })).toThrow(/200 MB/);
  });

  it("copies every snapshot before replying and removes partial deliveries on a later copy failure", () => {
    const item = request();
    for (const name of ["first.txt", "second.txt"]) {
      writeFileSync(join(writer.homeFolder, name), name);
      call(writer.id, "share_file", { path: name });
    }
    const card = history(db, item.conversationId, undefined, 1)[0];
    expect(card.data?.path).not.toBe(join(writer.homeFolder, "second.txt"));
    rmSync(String(card.data?.path)); // force the second copy to fail after the first succeeds
    team.turnEnded(item, done("Both files."));
    expect(getExchange(db, item.exchangeId!).status).toBe("failed");
    expect(readdirSync(join(researcher.homeFolder, "inbox"))).toEqual([]);
    expect(history(db, TEAM_ROOM_ID, undefined, 10).some((m) => m.body === "Both files.")).toBe(false);
  });

  it("retains a file card snapshot after a chat source is deleted", () => {
    const source = join(writer.homeFolder, "answer.txt");
    writeFileSync(source, "card contents");
    call(writer.id, "share_file", { path: source });
    rmSync(source);
    const card = history(db, writer.id, undefined, 1)[0];
    expect(readFileSync(String(card.data?.path), "utf8")).toBe("card contents");
  });

  it("removes the snapshot when the file card cannot be saved", () => {
    writeFileSync(join(writer.homeFolder, "answer.txt"), "card contents");
    db.exec("CREATE TRIGGER reject_file BEFORE INSERT ON messages WHEN NEW.kind = 'file' BEGIN SELECT RAISE(ABORT, 'card failed'); END");
    expect(() => call(writer.id, "share_file", { path: "answer.txt" })).toThrow("card failed");
    expect(history(db, writer.id, undefined, 10)).toEqual([]);
    expect(readdirSync(join(root, "shared-files"))).toEqual([]);
  });

  it("allows an ordinary project but rejects another bot reached through an ancestor project", () => {
    const project = tempRoot();
    try {
      writeFileSync(join(project, "notes.txt"), "project notes");
      updateBot(db, writer.id, { projectFolder: project });
      expect(call(writer.id, "share_file", { path: "notes.txt" })).toBe("Shared notes.txt.");
      // Simulate a legacy project selection that predates the project-folder guard.
      db.prepare("UPDATE bots SET project_folder = ? WHERE id = ?").run(root, writer.id);
      expect(() => call(writer.id, "share_file", { path: join(researcher.homeFolder, "notes.md") })).toThrow(/only share files from your own folders/);
    } finally {
      rmSync(project, { recursive: true, force: true });
    }
  });

  it("rolls back a reply and copied files and fails visibly when queuing the reply fails", () => {
    const item = request();
    writeFileSync(join(writer.homeFolder, "answer.txt"), "safe");
    call(writer.id, "share_file", { path: "answer.txt" });
    db.exec("CREATE TRIGGER reject_reply BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' BEGIN SELECT RAISE(ABORT, 'reply failed'); END");
    events.length = 0;
    expect(() => team.turnEnded(item, done("Reply text."))).toThrow("reply failed");
    expect(getExchange(db, item.exchangeId!).status).toBe("failed");
    expect(existsSync(join(researcher.homeFolder, "inbox", "answer.txt"))).toBe(false);
    expect(history(db, TEAM_ROOM_ID, undefined, 10).some((m) => m.body === "Reply text.")).toBe(false);
    expect(history(db, TEAM_ROOM_ID, undefined, 1)[0]).toMatchObject({ kind: "notice", body: expect.stringContaining("the reply could not be saved") });
    expect(events).not.toContainEqual(expect.objectContaining({ data: expect.objectContaining({ body: "Reply text." }) }));
  });
});
