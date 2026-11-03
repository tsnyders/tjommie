import { afterEach, expect, it, vi } from "vitest";
import { ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES } from "../../protocol/src/attachments";
import { documentRange, PdfRangeQueue } from "./document-ranges";
import { findMarkdownText, MARKDOWN_SECTION_BYTES, markdownSection } from "./markdown-document";

afterEach(() => vi.unstubAllGlobals());
function source(size: number, read: (begin: number, end: number) => Uint8Array) {
  const requests: number[] = [];
  const fetcher = vi.fn(async (_url: string, init: RequestInit) => {
    const match = /^bytes=(\d+)-(\d+)$/.exec(new Headers(init.headers).get("Range")!)!, begin = Number(match[1]), end = Number(match[2]) + 1;
    requests.push(end - begin); return new Response(read(begin, end) as Uint8Array<ArrayBuffer>, { status: 206, headers: { "content-range": `bytes ${begin}-${end - 1}/${size}`, "content-length": String(end - begin) } });
  });
  vi.stubGlobal("fetch", fetcher); return { requests, fetcher };
}
it("reads first and last sections of a 200MiB Markdown file without reading or assembling the intervening file", async () => {
  const { requests } = source(MAX_ATTACHMENT_BYTES, (begin, end) => new Uint8Array(end - begin).fill(97));
  const signal = new AbortController().signal;
  expect((await markdownSection("local", MAX_ATTACHMENT_BYTES, 0, signal)).text).toHaveLength(MARKDOWN_SECTION_BYTES);
  expect((await markdownSection("local", MAX_ATTACHMENT_BYTES, 6399, signal)).text).toHaveLength(MARKDOWN_SECTION_BYTES);
  expect(requests).toEqual([MARKDOWN_SECTION_BYTES, MARKDOWN_SECTION_BYTES + 3]);
});
it("preserves UTF-8 characters across every paged boundary and finds text crossing a boundary", async () => {
  const text = "a".repeat(MARKDOWN_SECTION_BYTES - 1) + "😀 café boundary" + "x".repeat(MARKDOWN_SECTION_BYTES), bytes = new TextEncoder().encode(text);
  source(bytes.length, (begin, end) => bytes.slice(begin, end));
  const signal = new AbortController().signal, sections: string[] = [];
  for (let section = 0; section < Math.ceil(bytes.length / MARKDOWN_SECTION_BYTES); section++) sections.push((await markdownSection("local", bytes.length, section, signal)).text);
  expect(sections.join("")).toBe(text);
  expect(await findMarkdownText("local", bytes.length, "a😀 café", 0, signal)).toEqual({ offset: MARKDOWN_SECTION_BYTES - 2, section: 0 });
  expect(await findMarkdownText("local", bytes.length, "boundary", 0, signal)).toMatchObject({ section: 1 });
});
it("rejects non-UTF8, range header/count corruption and an aborted read", async () => {
  source(2, () => Uint8Array.of(255, 255));
  await expect(markdownSection("local", 2, 0, new AbortController().signal)).rejects.toThrow();
  vi.stubGlobal("fetch", vi.fn(async () => new Response("bad", { status: 200 })));
  await expect(documentRange("local", 3, 0, 3, new AbortController().signal)).rejects.toThrow("no longer available");
  const abort = new AbortController(); abort.abort();
  await expect(markdownSection("local", 3, 0, abort.signal)).rejects.toMatchObject({ name: "AbortError" });
});
it("delivers all 800 full-size PDF callbacks serially, one exact chunk each, without the provisional queue/range cap", async () => {
  const { requests } = source(MAX_ATTACHMENT_BYTES, (begin, end) => new Uint8Array(end - begin).fill(Math.floor(begin / ATTACHMENT_CHUNK_BYTES) % 255));
  let delivered = 0; const failed = vi.fn();
  const queue = new PdfRangeQueue({ url: "local", size: MAX_ATTACHMENT_BYTES, signal: new AbortController().signal, fail: failed, deliver: (begin, bytes) => { expect(begin).toBe(delivered * ATTACHMENT_CHUNK_BYTES); expect(bytes.byteLength).toBe(ATTACHMENT_CHUNK_BYTES); delivered++; } });
  for (let begin = 0; begin < MAX_ATTACHMENT_BYTES; begin += ATTACHMENT_CHUNK_BYTES) queue.request(begin, begin + ATTACHMENT_CHUNK_BYTES);
  await queue.settled(); expect(delivered).toBe(800); expect(requests).toHaveLength(800); expect(Math.max(...requests)).toBe(ATTACHMENT_CHUNK_BYTES); expect(failed).not.toHaveBeenCalled();
});
it("fails duplicate/overlapping PDF requests and drops all queued callbacks after abort", async () => {
  const { fetcher } = source(ATTACHMENT_CHUNK_BYTES * 2, (begin, end) => new Uint8Array(end - begin));
  const abort = new AbortController(), deliver = vi.fn(), fail = vi.fn(), queue = new PdfRangeQueue({ url: "local", size: ATTACHMENT_CHUNK_BYTES * 2, signal: abort.signal, deliver, fail });
  queue.request(0, ATTACHMENT_CHUNK_BYTES); queue.request(0, ATTACHMENT_CHUNK_BYTES); await queue.settled();
  expect(fail).toHaveBeenCalledOnce(); expect(fetcher).not.toHaveBeenCalled(); expect(deliver).not.toHaveBeenCalled();
  const other = new PdfRangeQueue({ url: "local", size: ATTACHMENT_CHUNK_BYTES * 2, signal: abort.signal, deliver, fail });
  other.request(0, ATTACHMENT_CHUNK_BYTES); other.request(ATTACHMENT_CHUNK_BYTES, ATTACHMENT_CHUNK_BYTES * 2); abort.abort(); await other.settled(); expect(deliver).not.toHaveBeenCalled();
});
