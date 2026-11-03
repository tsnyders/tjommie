import { createHash } from "node:crypto";
import { existsSync, ftruncateSync, mkdirSync, openSync, closeSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES, type AttachmentRef } from "../../protocol/src/attachments.ts";
import { attachmentHandlers } from "./attachment-handlers.ts";
import { Attachments, attachmentContentType } from "./attachments.ts";
import { createBot, type StoredBot } from "./bots.ts";
import { addMessage } from "./chat.ts";
import { openDb, type Db } from "./db.ts";
import { addDevice, revokeDevice } from "./devices-store.ts";
import type { RequestContext } from "./request-context.ts";
import { tempRoot } from "./testing.ts";

let root: string, db: Db, service: Attachments, bot: StoredBot, controller: AbortController, context: RequestContext;
beforeEach(() => {
  root = tempRoot(); mkdirSync(join(root, "data")); mkdirSync(join(root, "shared-files", "share-fixture"), { recursive: true });
  db = openDb(join(root, "data", "tjommie.db"));
  // Unit fixture while the coordinator owns the shipped migration/integration gate.
  db.exec(`CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY, host_id TEXT NOT NULL, storage_name TEXT NOT NULL UNIQUE,
    original_name TEXT NOT NULL, byte_length INTEGER NOT NULL CHECK(byte_length >= 0 AND byte_length <= 209715200),
    content_type TEXT NOT NULL, sha256 TEXT NOT NULL, created_at TEXT NOT NULL
  ); CREATE TABLE IF NOT EXISTS message_attachments (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE, attachment_index INTEGER NOT NULL CHECK(attachment_index >= 0),
    attachment_id TEXT NOT NULL REFERENCES attachments(id), PRIMARY KEY(message_id,attachment_index)
  ); CREATE INDEX IF NOT EXISTS message_attachments_by_attachment ON message_attachments(attachment_id);`);
  bot = createBot(db, join(root, "bots"), { name: "Files", color: "blue", shape: "circle", instructions: "", provider: "codex", model: "synthetic" });
  controller = new AbortController(); context = { connectionId: "local-connection", principal: { kind: "local" }, signal: controller.signal };
  service = new Attachments({ db, root, hostId: "fixture-host" });
});
afterEach(async () => { vi.useRealTimers(); await service.close(); db.close(); });
function publish(bytes: string | Buffer = "immutable snapshot", name = "answer.md") {
  const source = join(root, "shared-files", "share-fixture", name); writeFileSync(source, bytes);
  const info = service.registerSnapshot(source, name);
  const message = addMessage(db, { conversationId: bot.id, author: bot.id, kind: "file", body: name, data: { path: source, name, size: Buffer.byteLength(bytes), attachmentId: info.id }, status: "done" });
  service.link(message.id, 0, info.id);
  return { source, info, ref: { messageId: message.id, attachmentIndex: 0, attachmentId: info.id } satisfies AttachmentRef };
}

describe("durable message-owned attachments", () => {
  it("serves the registered copy after the source is changed/deleted and returns bounded exact bytes", async () => {
    const { source, info, ref } = publish(Buffer.alloc(ATTACHMENT_CHUNK_BYTES + 3, 97)); writeFileSync(source, "replacement"); rmSync(source);
    expect(service.metadata(ref, context)).toEqual(info);
    const transfer = await service.open(ref, context);
    const first = await service.read({ transferId: transfer.transferId, offset: 0, length: ATTACHMENT_CHUNK_BYTES }, context);
    expect(Buffer.from(first.bytes, "base64")).toHaveLength(ATTACHMENT_CHUNK_BYTES); expect(first.eof).toBe(false);
    const last = await service.read({ transferId: transfer.transferId, offset: ATTACHMENT_CHUNK_BYTES, length: ATTACHMENT_CHUNK_BYTES }, context);
    expect(Buffer.from(last.bytes, "base64").toString()).toBe("aaa"); expect(last.eof).toBe(true); expect(last.revision).toBe(info.sha256);
    expect(JSON.stringify(transfer)).not.toContain(root);
  });
  it("handles a zero-byte delivered file and EOF without a fake byte", async () => {
    const { ref } = publish(""); const { transferId } = await service.open(ref, context);
    expect(await service.read({ transferId, offset: 0, length: 1 }, context)).toMatchObject({ offset: 0, bytes: "", eof: true });
  });
  it("requires trusted context and does not accept a path or identity in params", async () => {
    const { ref } = publish(); const handlers = attachmentHandlers(service);
    expect(() => handlers["attachments.metadata"](ref)).toThrow(/authorized/);
    await expect(handlers["attachments.open"](ref)).rejects.toMatchObject({ code: "forbidden" });
    expect(() => service.metadata({ ...ref, path: "C:\\private.txt" } as AttachmentRef, context)).toThrow();
    expect(() => service.metadata({ ...ref, attachmentId: "another-id" }, context)).toThrow(/unavailable/);
    db.prepare("UPDATE attachments SET host_id='another-host' WHERE id=?").run(ref.attachmentId!);
    expect(() => service.metadata(ref, context)).toThrow(/unavailable/);
  });
  it("limits parallel opens, serializes reads and isolates connection/principal handles", async () => {
    const { ref } = publish(Buffer.alloc(ATTACHMENT_CHUNK_BYTES, 1));
    const pending = [service.open(ref, context), service.open(ref, context)]; await expect(service.open(ref, context)).rejects.toMatchObject({ code: "busy" });
    const [first, second] = await Promise.all(pending);
    const read = service.read({ transferId: first.transferId, offset: 0, length: ATTACHMENT_CHUNK_BYTES }, context);
    await expect(service.read({ transferId: first.transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "busy" }); await read;
    const other = { ...context, connectionId: "new-connection" };
    await expect(service.read({ transferId: first.transferId, offset: 0, length: 1 }, other)).rejects.toMatchObject({ code: "forbidden" });
    await expect(service.closeTransfer(first.transferId, other)).rejects.toMatchObject({ code: "forbidden" });
    await service.closeTransfer(second.transferId, context); await service.closeTransfer(second.transferId, context);
  });
  it("rejects every malformed range without ending a valid transfer", async () => {
    const { ref } = publish("abcd"), { transferId } = await service.open(ref, context);
    for (const range of [{ offset: -1, length: 1 }, { offset: 5, length: 1 }, { offset: 0.5, length: 1 }, { offset: 0, length: 0 }, { offset: 0, length: ATTACHMENT_CHUNK_BYTES + 1 }]) await expect(service.read({ transferId, ...range }, context)).rejects.toMatchObject({ code: "bad_params" });
    expect((await service.read({ transferId, offset: 4, length: 1 }, context)).bytes).toBe("");
  });
  it("aborts on disconnect and checks device revocation on each operation", async () => {
    const { ref } = publish(); const first = await service.open(ref, context); controller.abort();
    await expect(service.read({ transferId: first.transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "forbidden" });
    const device = addDevice(db, { publicKey: Buffer.alloc(32, 7).toString("base64url"), name: "Synthetic device" });
    const paired = { connectionId: "paired", principal: { kind: "device", deviceId: device.id } as const, signal: new AbortController().signal };
    const remote = await service.open(ref, paired); revokeDevice(db, device.id);
    await expect(service.read({ transferId: remote.transferId, offset: 0, length: 1 }, paired)).rejects.toMatchObject({ code: "forbidden" });
    expect(() => service.metadata(ref, paired)).toThrow(/authorized/);
  });
  it("invalidates reads when message ownership disappears and retains leased bytes until close", async () => {
    const { ref, info } = publish(), { transferId } = await service.open(ref, context), file = join(root, "shared-files", "attachments", `${info.id}.bin`);
    db.prepare("DELETE FROM messages WHERE id=?").run(ref.messageId); service.cleanup(); expect(existsSync(file)).toBe(true);
    await expect(service.read({ transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "unavailable" });
    await service.closeTransfer(transferId, context); service.cleanup(); expect(existsSync(file)).toBe(false);
  });
  it("detects corrupt content before releasing bytes and changed content before subsequent reads", async () => {
    const { ref, info } = publish("first"), file = join(root, "shared-files", "attachments", `${info.id}.bin`);
    writeFileSync(file, "other"); const corrupt = await service.open(ref, context); await expect(service.read({ transferId: corrupt.transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "corrupt" });
    writeFileSync(file, "first"); const { transferId } = await service.open(ref, context);
    writeFileSync(file, "longer"); await expect(service.read({ transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "corrupt" });
  });
  it("cancels an in-progress first read through the returned transfer handle", async () => {
    const { ref } = publish(Buffer.alloc(4 * 1024 * 1024, 7)), { transferId } = await service.open(ref, context);
    const reading = service.read({ transferId, offset: 0, length: 1 }, context), rejected = expect(reading).rejects.toMatchObject({ code: "forbidden" });
    await service.closeTransfer(transferId, context); await rejected;
    expect((await service.open(ref, context)).attachment.id).toBe(ref.attachmentId);
  });
  it("adopts only known inbox/staging legacy files and preserves the original inbox copy", () => {
    const inbox = join(bot.homeFolder, "inbox", "old.md"); writeFileSync(inbox, "legacy");
    const outside = join(root, "project.md"); writeFileSync(outside, "outside");
    const ids = [inbox, outside, join(bot.homeFolder, "inbox", "missing.md")].map((path) => addMessage(db, { conversationId: bot.id, author: bot.id, kind: "file", body: "legacy", data: { path, name: "old.md", size: 6 }, status: "done" }).id);
    expect(service.backfillLegacy()).toEqual({ registered: 1, unavailable: 2 });
    expect(service.metadata({ messageId: ids[0], attachmentIndex: 0 }, context).sha256).toBe(createHash("sha256").update("legacy").digest("hex"));
    expect(readFileSync(inbox, "utf8")).toBe("legacy"); expect(service.backfillLegacy()).toEqual({ registered: 0, unavailable: 2 });
    for (const id of ids.slice(1)) expect(() => service.metadata({ messageId: id, attachmentIndex: 0 }, context)).toThrow(/unavailable/);
  });
  it("refuses linked legacy parents and oversized snapshots without importing outside data", () => {
    const outside = join(root, "outside"); mkdirSync(outside); writeFileSync(join(outside, "private.md"), "outside");
    const linked = join(bot.homeFolder, "inbox", "linked"); symlinkSync(outside, linked, "junction");
    const message = addMessage(db, { conversationId: bot.id, author: bot.id, kind: "file", body: "legacy", data: { path: join(linked, "private.md"), name: "private.md", size: 7 }, status: "done" });
    expect(() => service.metadata({ messageId: message.id, attachmentIndex: 0 }, context)).toThrow(/unavailable/);
    const large = join(root, "shared-files", "share-fixture", "large.bin"), fd = openSync(large, "wx"); ftruncateSync(fd, MAX_ATTACHMENT_BYTES + 1); closeSync(fd);
    expect(() => service.registerSnapshot(large, "large.bin")).toThrow(/too large/);
  });
  it("rejects storage traversal and a legacy file whose recorded size no longer matches", () => {
    const { ref, info } = publish(); db.prepare("UPDATE attachments SET storage_name='../../private.txt' WHERE id=?").run(info.id);
    expect(() => service.metadata(ref, context)).toThrow(/record is invalid/);
    const file = join(bot.homeFolder, "inbox", "changed.md"); writeFileSync(file, "modified after delivery");
    const message = addMessage(db, { conversationId: bot.id, author: bot.id, kind: "file", body: "old", data: { path: file, name: "changed.md", size: 3 }, status: "done" });
    expect(() => service.metadata({ messageId: message.id, attachmentIndex: 0 }, context)).toThrow(/unavailable/);
  });
  it("closes idle leases and can reopen without leaking transfer capacity", async () => {
    const { ref } = publish(), first = await service.open(ref, context); vi.useFakeTimers(); await vi.advanceTimersByTimeAsync(5 * 60 * 1000 + 1);
    await expect(service.read({ transferId: first.transferId, offset: 0, length: 1 }, context)).rejects.toMatchObject({ code: "expired" });
    vi.useRealTimers(); expect((await service.open(ref, context)).attachment.id).toBe(ref.attachmentId);
  });
  it("uses content signatures and keeps active formats download-only", () => {
    expect(attachmentContentType("fake.png", Buffer.from("<script>alert(1)</script>"))).toBe("application/octet-stream");
    expect(attachmentContentType("readme.md", Buffer.from("# Notes"))).toBe("text/markdown");
    expect(attachmentContentType("file.pdf", Buffer.from("%PDF-1.7"))).toBe("application/pdf");
    expect(attachmentContentType("drawing.svg", Buffer.from("<svg/>"))).toBe("application/octet-stream");
  });
});
