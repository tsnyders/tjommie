import { describe, expect, it, vi } from "vitest";
import { ATTACHMENT_CHUNK_BYTES, type AttachmentInfo } from "../../protocol/src/attachments.ts";
import { AttachmentProtocol, AttachmentReadRequests, attachmentHttpRange } from "./attachment-protocol.ts";

const metadata = (size: number): AttachmentInfo => ({ id: "41f8bac7-358c-48cf-9f48-a3a358804da6", hostId: "host", name: "file.bin", size, contentType: "application/octet-stream", sha256: "a".repeat(64) });
describe("scoped attachment protocol", () => {
  it("streams an open-ended range using bounded pulls and accurate headers", async () => {
    const protocol = new AttachmentProtocol(), reads: number[] = [], size = ATTACHMENT_CHUNK_BYTES * 2 + 3;
    const { url } = protocol.create("window", metadata(size), async (_at, length) => { reads.push(length); return new Uint8Array(length).fill(7); });
    const response = await protocol.serve(new Request(url, { headers: { Range: "bytes=1-" } }), "window");
    expect(response.status).toBe(206); expect(response.headers.get("content-range")).toBe(`bytes 1-${size - 1}/${size}`); expect(response.headers.get("cache-control")).toBe("no-store");
    expect((await response.arrayBuffer()).byteLength).toBe(size - 1); expect(reads).toEqual([ATTACHMENT_CHUNK_BYTES, ATTACHMENT_CHUNK_BYTES, 2]);
    const head = await protocol.serve(new Request(url, { method: "HEAD" }), "window"); expect(head.status).toBe(200); expect(await head.text()).toBe(""); expect(reads).toHaveLength(3);
  });
  it("rejects wrong-owner/unknown/malformed ranges and does not serve app resources", async () => {
    const protocol = new AttachmentProtocol(), { url } = protocol.create("window", metadata(10), async (_at, count) => new Uint8Array(count));
    expect((await protocol.serve(new Request(url), "another")).status).toBe(403);
    for (const range of ["bytes=10-", "bytes=-0", "bytes=3-1", "bytes=0-1,4-5", "bytes=1.5-2", "bytes=9007199254740992-"]) expect((await protocol.serve(new Request(url, { headers: { Range: range } }), "window")).status).toBe(416);
    expect((await protocol.serve(new Request("app://tjommie/index.html"), "window")).status).toBe(404);
    expect((await protocol.serve(new Request(url, { method: "POST" }), "window")).status).toBe(405);
    expect(attachmentHttpRange("bytes=-20", 10)).toEqual({ start: 0, end: 9 }); expect(attachmentHttpRange("bytes=8-99", 10)).toEqual({ start: 8, end: 9 });
  });
  it("stops a live stream after disconnect and refuses later requests", async () => {
    const protocol = new AttachmentProtocol(), abort = new AbortController(), { url } = protocol.create("window", metadata(ATTACHMENT_CHUNK_BYTES * 2), async (_at, count) => new Uint8Array(count), abort.signal);
    const response = await protocol.serve(new Request(url), "window"), reader = response.body!.getReader(); expect((await reader.read()).value).toHaveLength(ATTACHMENT_CHUNK_BYTES);
    abort.abort(); await expect(reader.read()).rejects.toThrow(/closed/); expect((await protocol.serve(new Request(url), "window")).status).toBe(404);
  });
  it("does not pass short or oversized bridge responses into a media element", async () => {
    const protocol = new AttachmentProtocol(), { url } = protocol.create("window", metadata(10), async () => new Uint8Array(9));
    const response = await protocol.serve(new Request(url), "window"); await expect(response.arrayBuffer()).rejects.toThrow(/Invalid preview bytes/);
  });
  it("binds bridge replies to owner/preview/request and rejects replay or a malformed chunk", async () => {
    const broker = new AttachmentReadRequests(), emit = vi.fn();
    const pending = broker.request("window", "preview", 0, 3, emit), request = emit.mock.calls[0][0];
    expect(() => broker.respond("another", "preview", request.requestId, "YWJj")).toThrow(/authorized/);
    broker.respond("window", "preview", request.requestId, "YWJj"); expect(await pending).toEqual(Buffer.from("abc"));
    expect(() => broker.respond("window", "preview", request.requestId, "YWJj")).toThrow(/authorized/);
    const corrupt = broker.request("window", "preview", 0, 3, emit), rejected = expect(corrupt).rejects.toThrow(/Invalid/);
    broker.respond("window", "preview", emit.mock.calls[1][0].requestId, "YQ=="); await rejected;
  });
  it("cancels queued bridge requests and enforces a bounded response deadline", async () => {
    const broker = new AttachmentReadRequests(), emit = vi.fn(), pending = broker.request("window", "preview", 0, 1, emit), cancelled = expect(pending).rejects.toThrow(/closed/);
    broker.cancel("window"); await cancelled;
    vi.useFakeTimers();
    try { const stalled = broker.request("window", "preview", 0, 1, emit), expired = expect(stalled).rejects.toThrow(/did not answer/); await vi.advanceTimersByTimeAsync(30000); await expired; }
    finally { vi.useRealTimers(); }
  });
});
