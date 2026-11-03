import { randomUUID } from "node:crypto";
import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo, type AttachmentNativeRead } from "../../protocol/src/attachments.ts";
import { decodeAttachmentChunk } from "./attachment-save.ts";

export function attachmentHttpRange(header: string | null, size: number): { start: number; end: number } | null {
  if (header === null) return null;
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || !size || !match[1] && !match[2]) throw new RangeError("Invalid byte range.");
  const first = match[1] ? Number(match[1]) : null, last = match[2] ? Number(match[2]) : null;
  if (first !== null && !Number.isSafeInteger(first) || last !== null && !Number.isSafeInteger(last)) throw new RangeError("Invalid byte range.");
  const start = first ?? Math.max(0, size - last!), end = first === null || last === null ? size - 1 : Math.min(last, size - 1);
  if (start < 0 || start >= size || end < start || first === null && last === 0) throw new RangeError("Invalid byte range."); return { start, end };
}
type Entry = { owner: string; info: AttachmentInfo; read: (offset: number, length: number) => Promise<Uint8Array>; closed: boolean; signal?: AbortSignal; abort: () => void; tail: Promise<void>; queued: number };
const headers = { "cache-control": "no-store", "x-content-type-options": "nosniff", "content-security-policy": "default-src 'none'; sandbox" };
const safeMime = (type: string) => /^(image\/(png|jpeg|gif|webp)|audio\/(wav|mpeg|mp4|ogg)|video\/(mp4|webm)|application\/pdf|text\/markdown)$/.test(type) ? type : "application/octet-stream";

// Register only in the trusted app session. Bot browser sessions must not receive this handler.
export class AttachmentProtocol {
  #entries = new Map<string, Entry>();
  create(owner: string, metadata: AttachmentInfo, read: Entry["read"], signal?: AbortSignal): { handle: string; url: string } {
    if (!owner || signal?.aborted) throw new Error("The preview connection is closed.");
    if (this.#entries.size >= 16 || [...this.#entries.values()].filter((entry) => entry.owner === owner).length >= 2) throw new Error("Two previews are already open.");
    const handle = randomUUID(), entry: Entry = { owner, info: AttachmentInfo.parse(metadata), read, closed: false, signal, abort: () => this.close(owner, handle), tail: Promise.resolve(), queued: 0 };
    this.#entries.set(handle, entry); signal?.addEventListener("abort", entry.abort, { once: true });
    return { handle, url: `app://tjommie/_attachments/${handle}` };
  }
  close(owner: string, handle: string): void {
    const entry = this.#entries.get(handle); if (!entry) return;
    if (entry.owner !== owner) throw new Error("This preview is not authorized.");
    this.#entries.delete(handle); entry.closed = true; entry.signal?.removeEventListener("abort", entry.abort);
  }
  closeOwner(owner: string): void { for (const [id, entry] of this.#entries) if (entry.owner === owner) this.close(owner, id); }
  async #read(entry: Entry, offset: number, length: number): Promise<Uint8Array> {
    if (entry.queued >= 2) throw new Error("Too many preview reads."); entry.queued++;
    const previous = entry.tail; let release: () => void = () => {};
    entry.tail = new Promise((resolve) => { release = resolve; });
    try {
      await previous;
      if (entry.closed || entry.signal?.aborted) throw new Error("The preview was closed.");
      const bytes = await entry.read(offset, length);
      if (entry.closed || entry.signal?.aborted || bytes.length !== length || bytes.length > ATTACHMENT_CHUNK_BYTES) throw new Error("Invalid preview bytes."); return bytes;
    } finally { entry.queued--; release(); }
  }
  async serve(request: Request, owner: string): Promise<Response> {
    const url = new URL(request.url), handle = /^\/_attachments\/([0-9a-f-]{36})$/.exec(url.pathname)?.[1], entry = handle ? this.#entries.get(handle) : undefined;
    if (url.protocol !== "app:" || url.host !== "tjommie" || url.username || url.password || url.search || url.hash || !entry) return new Response(null, { status: 404, headers });
    if (entry.owner !== owner || entry.closed || entry.signal?.aborted) return new Response(null, { status: 403, headers });
    if (request.method !== "GET" && request.method !== "HEAD") return new Response(null, { status: 405, headers });
    const size = entry.info.size; let range: ReturnType<typeof attachmentHttpRange>;
    try { range = request.method === "HEAD" ? null : attachmentHttpRange(request.headers.get("range"), size); }
    catch { return new Response(null, { status: 416, headers: { ...headers, "content-range": `bytes */${size}` } }); }
    const start = range?.start ?? 0, end = range?.end ?? size - 1;
    const resultHeaders = { ...headers, "content-type": safeMime(entry.info.contentType), "accept-ranges": "bytes", "content-length": String(Math.max(0, end - start + 1)), ...(range ? { "content-range": `bytes ${start}-${end}/${size}` } : {}) };
    if (request.method === "HEAD") return new Response(null, { status: 200, headers: resultHeaders });
    let at = start, cancelled = false;
    const stream = new ReadableStream<Uint8Array>({
      pull: async (controller) => {
        if (cancelled) return;
        if (entry.closed || entry.signal?.aborted) { controller.error(new Error("The preview was closed.")); return; }
        if (at > end) { controller.close(); return; }
        try { const bytes = await this.#read(entry, at, Math.min(ATTACHMENT_CHUNK_BYTES, end - at + 1)); if (!cancelled) { at += bytes.length; controller.enqueue(bytes); } }
        catch (error) { if (!cancelled) controller.error(error); }
      },
      cancel: () => { cancelled = true; },
    }, { highWaterMark: 0 });
    return new Response(stream, { status: range ? 206 : 200, headers: resultHeaders });
  }
}

type PendingRead = { owner: string; previewId: string; length: number; accept: (bytes: Uint8Array) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> };
export class AttachmentReadRequests {
  #pending = new Map<string, PendingRead>();
  request(owner: string, previewId: string, offset: number, length: number, emit: (request: AttachmentNativeRead) => void): Promise<Uint8Array> {
    if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(length) || length < 1 || length > ATTACHMENT_CHUNK_BYTES || this.#pending.size >= 32) return Promise.reject(new Error("Invalid preview read."));
    const requestId = randomUUID();
    return new Promise((accept, reject) => {
      const timer = setTimeout(() => { this.#pending.delete(requestId); reject(new Error("The file host did not answer.")); }, 30000); timer.unref();
      this.#pending.set(requestId, { owner, previewId, length, accept, reject, timer });
      try { emit({ previewId, requestId, offset, length }); }
      catch { this.#pending.delete(requestId); clearTimeout(timer); reject(new Error("The preview connection is closed.")); }
    });
  }
  respond(owner: string, previewId: string, requestId: string, encoded: unknown): void {
    const pending = this.#pending.get(requestId);
    if (!pending || pending.owner !== owner || pending.previewId !== previewId) throw new Error("This preview response is not authorized.");
    this.#pending.delete(requestId); clearTimeout(pending.timer);
    try { if (encoded === null) throw new Error("The shared file could not be read."); const bytes = decodeAttachmentChunk(encoded); if (bytes.length !== pending.length) throw new Error("Invalid preview bytes."); pending.accept(bytes); }
    catch (error) { pending.reject(error instanceof Error ? error : new Error("Invalid preview bytes.")); }
  }
  cancel(owner: string, previewId?: string): void {
    for (const [id, pending] of this.#pending) if (pending.owner === owner && (previewId === undefined || pending.previewId === previewId)) {
      this.#pending.delete(id); clearTimeout(pending.timer); pending.reject(new Error("The preview connection is closed."));
    }
  }
}
