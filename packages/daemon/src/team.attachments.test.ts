import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { Attachments } from "./attachments.ts";
import { createBot, updateBot, type StoredBot } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { enqueue, type WorkItem } from "./queue.ts";
import { Team } from "./team.ts";
import { tempRoot } from "./testing.ts";
import { ExchangePostData, FileData } from "../../protocol/src/model.ts";

let root: string, db: Db, service: Attachments, team: Team, writer: StoredBot, asker: StoredBot, folder: string;
let work: Map<string, WorkItem>, delivered: WorkItem[], published: string[];
const context = { connectionId: "local-fixture", principal: { kind: "local" as const }, signal: new AbortController().signal };
const done = { outcome: "done" as const, text: "Done", textMessageId: null, error: null };
beforeEach(() => {
  root = tempRoot(); mkdirSync(join(root, "shared-files")); db = openDb(join(root, "team.db"));
  writer = createBot(db, join(root, "bots"), { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "synthetic" });
  asker = createBot(db, join(root, "bots"), { name: "Asker", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "synthetic", canMessage: [writer.id] });
  writer = updateBot(db, writer.id, { canMessage: [asker.id] });
  service = new Attachments({ db, root, hostId: "fixture-host" });
  work = new Map(); delivered = []; published = []; folder = writer.homeFolder;
  team = new Team({ db, attachments: service, effectiveFolder: () => folder, currentWork: (id) => work.get(id) ?? null, stateOf: () => "idle", stopItem: () => {}, deliver: (item) => { delivered.push(item); },
    broadcast: (event, data) => {
      if (event === "message.added" && "kind" in data && data.kind === "file") {
        const metadata = service.metadata({ messageId: data.id, attachmentIndex: 0 }, context); published.push(metadata.id);
      }
    },
  });
});
afterEach(async () => { await service.close(); db.close(); rmSync(root, { recursive: true, force: true }); });
const call = (id: string, name: string, input: unknown) => { const tool = team.tools().find((entry) => entry.name === name)!; return tool.run(id, tool.input.parse(input)); };

it("publishes linked immutable bytes before announcing a delivered file", async () => {
  writeFileSync(join(folder, "answer.md"), "# Answer"); call(writer.id, "share_file", { path: "answer.md" });
  const message = history(db, writer.id, undefined, 200)[0], ref = { messageId: message.id, attachmentIndex: 0 };
  const metadata = service.metadata(ref, context); expect(published).toEqual([metadata.id]);
  expect(message.data).toMatchObject({ attachmentId: metadata.id, contentType: "text/markdown" });
  writeFileSync(join(folder, "answer.md"), "replacement");
  const opened = await service.open(ref, context), chunk = await service.read({ transferId: opened.transferId, offset: 0, length: 100 }, context);
  expect(Buffer.from(chunk.bytes, "base64").toString()).toBe("# Answer");
});

it("shares from the validated task folder and refuses a sibling managed folder", () => {
  folder = join(root, "workspaces", "owned"); mkdirSync(folder, { recursive: true });
  const sibling = join(root, "workspaces", "other"); mkdirSync(sibling); writeFileSync(join(sibling, "private.md"), "private");
  work.set(writer.id, enqueue(db, { botId: writer.id, source: "task", text: "task", taskAttemptId: "attempt", taskRevision: 1, executionContextKey: "context" }));
  writeFileSync(join(folder, "answer.md"), "task result");
  expect(call(writer.id, "share_file", { path: "answer.md" })).toBe("Shared answer.md.");
  expect(() => call(writer.id, "share_file", { path: join(sibling, "private.md") })).toThrow();
});

it("preserves task reply context and attachment identity in exchange room copies", () => {
  work.set(asker.id, enqueue(db, { botId: asker.id, source: "task", text: "task", taskAttemptId: "attempt", taskRevision: 3, executionContextKey: "exact-context" }));
  call(asker.id, "message_bot", { bot: "Writer", message: "Help" });
  const request = delivered.at(-1)!; work.set(writer.id, request);
  writeFileSync(join(folder, "answer.md"), "answer"); call(writer.id, "share_file", { path: "answer.md" });
  const original = history(db, request.conversationId, undefined, 200).find((entry) => entry.kind === "file")!;
  team.turnEnded(request, done);
  expect(delivered.at(-1)).toMatchObject({ botId: asker.id, source: "bot_reply", taskAttemptId: "attempt", taskRevision: 3, executionContextKey: "exact-context" });
  const room = history(db, "team", undefined, 200).find((entry) => Array.isArray(entry.data?.files))!;
  expect(service.metadata({ messageId: room.id, attachmentIndex: 0 }, context).id).toBe(original.data?.attachmentId);
});

const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR5sAAAAASUVORK5CYII=", "base64");
it("publishes explicit companion ownership atomically and revokes it with its parent message", async () => {
  writeFileSync(join(folder, "answer.md"), "# Answer\n![Chart](images/chart.png)"); writeFileSync(join(folder, "chart.png"), png);
  call(writer.id, "share_file", { path: "answer.md", companions: [{ name: "./images/chart.png", path: "chart.png" }] });
  const messages = history(db, writer.id, undefined, 200), message = messages.find((item) => item.kind === "file" && item.data?.name === "answer.md")!;
  const companion = FileData.parse(message.data).companions![0];
  expect(companion.name).toBe("images/chart.png"); expect(companion.attachmentIndex).toBe(1); expect(JSON.stringify(companion)).not.toContain(root);
  const ref = { messageId: message.id, attachmentIndex: companion.attachmentIndex, attachmentId: companion.attachmentId };
  const opened = await service.open(ref, context); expect(opened.attachment.contentType).toBe("image/png");
  expect(Buffer.from((await service.read({ transferId: opened.transferId, offset: 0, length: 100 }, context)).bytes, "base64")).toEqual(png);
  expect(() => service.metadata({ ...ref, attachmentIndex: 42 }, context)).toThrow("unavailable");
  expect(() => service.metadata({ ...ref, attachmentId: String(message.data?.attachmentId) }, context)).toThrow("unavailable");
  db.prepare("DELETE FROM messages WHERE id=?").run(message.id); service.cleanup();
  await expect(service.read({ transferId: opened.transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "unavailable" });
  await service.closeTransfer(opened.transferId, context);
});

it("remaps room companion indexes without changing IDs across delivery and restart", async () => {
  call(asker.id, "message_bot", { bot: "Writer", message: "Help" }); const request = delivered.at(-1)!; work.set(writer.id, request);
  writeFileSync(join(folder, "answer.md"), "![Chart](chart.png)"); writeFileSync(join(folder, "chart.png"), png);
  call(writer.id, "share_file", { path: "answer.md", companions: [{ name: "chart.png", path: "chart.png" }] });
  const original = history(db, request.conversationId, undefined, 200).find((entry) => entry.kind === "file" && entry.data?.name === "answer.md")!;
  const companion = FileData.parse(original.data).companions![0]; team.turnEnded(request, done);
  const room = history(db, "team", undefined, 200).find((entry) => Array.isArray(entry.data?.files))!;
  const files = ExchangePostData.parse(room.data).files!, roomCompanion = files[0].companions![0];
  expect(files).toHaveLength(2); expect(roomCompanion).toEqual({ ...companion, attachmentIndex: 2 });
  expect(service.metadata({ messageId: room.id, attachmentIndex: roomCompanion.attachmentIndex }, context).id).toBe(companion.attachmentId);
  expect(service.metadata({ messageId: room.id, attachmentIndex: 1 }, context).id).toBe(companion.attachmentId);
  await service.close(); service = new Attachments({ db, root, hostId: "fixture-host" }); service.backfillLegacy(); service.cleanup();
  expect(service.metadata({ messageId: room.id, attachmentIndex: roomCompanion.attachmentIndex }, context).id).toBe(companion.attachmentId);
});

it("refuses companion escape, duplicate names and active formats without publishing a partial bundle", () => {
  writeFileSync(join(folder, "answer.md"), "# Answer"); writeFileSync(join(folder, "chart.png"), png); writeFileSync(join(folder, "active.svg"), "<svg/>");
  const outside = join(root, "outside.png"); writeFileSync(outside, png);
  for (const companions of [[{ name: "outside.png", path: outside }], [{ name: "../chart.png", path: "chart.png" }], [{ name: "chart.png", path: "chart.png" }, { name: "./chart.png", path: "chart.png" }], [{ name: "active.svg", path: "active.svg" }]]) {
    expect(() => call(writer.id, "share_file", { path: "answer.md", companions })).toThrow();
    expect(history(db, writer.id, undefined, 200)).toHaveLength(0);
    expect((db.prepare("SELECT COUNT(*) AS n FROM attachments").get() as { n: number }).n).toBe(0);
  }
});
