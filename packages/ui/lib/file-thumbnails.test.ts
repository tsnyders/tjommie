// @vitest-environment happy-dom
import type { Client } from "@tjommie/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AttachmentInfo, AttachmentNativeApi } from "../../protocol/src/attachments";
import { pauseThumbnailWork, queueThumbnail, rasterDimensions, readFileThumbnail, thumbnailEligible, THUMBNAIL_IMAGE_BYTES, THUMBNAIL_PDF_FILE_BYTES, THUMBNAIL_VIDEO_BYTES } from "./file-thumbnails";

const fixture = vi.hoisted(() => ({ preview: vi.fn(), pdf: vi.fn(), destroy: vi.fn(), workerDestroy: vi.fn(), cleanup: vi.fn(), render: vi.fn(), page: vi.fn() }));
vi.mock("./attachment-transfer", async (original) => ({ ...await original<typeof import("./attachment-transfer")>(), previewAttachment: fixture.preview }));
vi.mock("http://localhost:3000/pdfjs/build/pdf.mjs", () => ({
  GlobalWorkerOptions: {},
  PDFWorker: class { destroy() { fixture.workerDestroy(); } },
  PDFDataRangeTransport: class { onDataRange() {} },
  getDocument: fixture.pdf,
}));
const client = {} as Pick<Client, "call">, native = {} as AttachmentNativeApi, reference = { messageId: "message", attachmentIndex: 0 };
const url = "app://tjommie/_attachments/624d43d7-1a49-4d79-8d1d-85d846cfbd8e";
let close = vi.fn(async () => {});
function info(contentType: string, size: number): AttachmentInfo { return { id: "624d43d7-1a49-4d79-8d1d-85d846cfbd8e", hostId: "storage-host", name: "fixture", contentType, size, sha256: "a".repeat(64) }; }
function source(contentType: string, bytes: Uint8Array, size = bytes.length) {
  fixture.preview.mockResolvedValue({ attachment: info(contentType, size), url, close });
  const fetcher = vi.fn(async (_url: string, options: RequestInit) => {
    const match = /^bytes=(\d+)-(\d+)$/.exec((options.headers as Record<string, string>).Range)!;
    const begin = Number(match[1]), end = Number(match[2]) + 1;
    return new Response(bytes.slice(begin, end), { status: 206, headers: { "content-range": `bytes ${begin}-${end - 1}/${size}`, "content-length": String(end - begin) } });
  }); vi.stubGlobal("fetch", fetcher); return fetcher;
}
function png(width: number, height: number, length = 24): Uint8Array {
  const bytes = new Uint8Array(length); bytes.set([137, 80, 78, 71, 13, 10, 26, 10]); bytes.set(new TextEncoder().encode("IHDR"), 12);
  const view = new DataView(bytes.buffer); view.setUint32(16, width); view.setUint32(20, height); return bytes;
}
beforeEach(() => {
  close = vi.fn(async () => {}); fixture.preview.mockReset(); fixture.pdf.mockReset(); fixture.destroy.mockReset(); fixture.workerDestroy.mockReset(); fixture.cleanup.mockReset(); fixture.render.mockReset(); fixture.page.mockReset();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers(); });

it("admits ordinary phone photos and rejects large automatic resources", () => {
  expect(thumbnailEligible("image/jpeg", 12 * 1024 * 1024)).toBe(true);
  expect(thumbnailEligible("image/jpeg", THUMBNAIL_IMAGE_BYTES + 1)).toBe(false);
  expect(thumbnailEligible("video/mp4", THUMBNAIL_VIDEO_BYTES + 1)).toBe(false);
  expect(thumbnailEligible("application/pdf", THUMBNAIL_PDF_FILE_BYTES)).toBe(true);
  expect(thumbnailEligible("application/pdf", 200 * 1024 * 1024)).toBe(false);
  expect(thumbnailEligible("text/markdown", 200 * 1024 * 1024)).toBe(true);
  expect(thumbnailEligible("audio/wav", 1024)).toBe(false);
});
it("reads only the actual first4KiB of a200MiB document and keeps markup literal", async () => {
  const bytes = new TextEncoder().encode("# Actual report\n<img src='https://invalid.example/private'>\n".padEnd(4096, "x")), fetcher = source("text/markdown", bytes, 200 * 1024 * 1024);
  const result = await readFileThumbnail(client, reference, native, new AbortController().signal);
  expect(result).toHaveProperty("text", expect.stringContaining("# Actual report\n<img"));
  expect(fetcher).toHaveBeenCalledOnce(); expect(fetcher.mock.calls[0][1].headers).toEqual({ Range: "bytes=0-4095" }); expect(close).toHaveBeenCalledOnce();
});
it("rejects duplicate range lengths and closes the secure preview", async () => {
  fixture.preview.mockResolvedValue({ attachment: info("text/plain", 2), url, close });
  vi.stubGlobal("fetch", vi.fn(async () => new Response("hi", { status: 206, headers: { "content-range": "bytes0-1/2", "content-length": "2,2" } })));
  await expect(readFileThumbnail(client, reference, native, new AbortController().signal)).rejects.toThrow(); expect(close).toHaveBeenCalledOnce();
});
it("decodes bounded12MP images after exact chunk reads and releases the bitmap and native handle", async () => {
  const fetcher = source("image/png", png(4000, 3000, 600_000)), bitmapClose = vi.fn();
  const decode = vi.fn(async () => ({ width: 240, height: 180, close: bitmapClose })); vi.stubGlobal("createImageBitmap", decode);
  const result = await readFileThumbnail(client, reference, native, new AbortController().signal);
  expect(result).toHaveProperty("canvas"); expect(fetcher.mock.calls.map(([, options]) => (options.headers as Record<string, string>).Range)).toEqual(["bytes=0-262143", "bytes=262144-524287", "bytes=524288-599999"]);
  expect(decode).toHaveBeenCalledWith(expect.any(Blob), { resizeWidth: 240, resizeHeight: 180 }); expect(bitmapClose).toHaveBeenCalledOnce(); expect(close).toHaveBeenCalledOnce();
});
it("checks raster pixel limits before decode, including JPEG/GIF/WebP dimensions", async () => {
  expect(rasterDimensions(png(4000, 3000), "image/png")).toEqual({ width: 4000, height: 3000 });
  const gif = new Uint8Array([71, 73, 70, 56, 57, 97, 128, 2, 104, 1]); expect(rasterDimensions(gif, "image/gif")).toEqual({ width: 640, height: 360 });
  const jpeg = new Uint8Array([255, 216, 255, 192, 0, 7, 8, 1, 104, 2, 128]); expect(rasterDimensions(jpeg, "image/jpeg")).toEqual({ width: 640, height: 360 });
  const webp = new Uint8Array(30); webp.set(new TextEncoder().encode("RIFF")); webp.set(new TextEncoder().encode("WEBPVP8X"), 8); webp[24] = 127; webp[25] = 2; webp[27] = 103; webp[28] = 1;
  expect(rasterDimensions(webp, "image/webp")).toEqual({ width: 640, height: 360 });
  source("image/png", png(100_000, 100_000)); const decode = vi.fn(); vi.stubGlobal("createImageBitmap", decode);
  await expect(readFileThumbnail(client, reference, native, new AbortController().signal)).rejects.toThrow(/dimensions/); expect(decode).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledOnce();
});
it("closes a bitmap that finishes decoding after cancellation without drawing it", async () => {
  source("image/png", png(640, 360)); let finish!: (value: ImageBitmap) => void;
  vi.stubGlobal("createImageBitmap", vi.fn(() => new Promise<ImageBitmap>((resolve) => { finish = resolve; })));
  const abort = new AbortController(), result = readFileThumbnail(client, reference, native, abort.signal);
  await vi.waitFor(() => expect(finish).toBeTypeOf("function")); abort.abort(); let retired = false; void result.then(() => { retired = true; }, () => { retired = true; });
  await Promise.resolve(); expect(retired).toBe(false);
  const bitmapClose = vi.fn(); finish({ close: bitmapClose } as unknown as ImageBitmap); await expect(result).rejects.toMatchObject({ name: "AbortError" }); expect(bitmapClose).toHaveBeenCalledOnce(); expect(close).toHaveBeenCalledOnce();
});
it("renders only PDF page1 through local ranges and destroys its worker", async () => {
  source("application/pdf", new Uint8Array(1000));
  fixture.render.mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() });
  fixture.page.mockResolvedValue({ getViewport: ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale }), render: fixture.render, cleanup: fixture.cleanup });
  fixture.pdf.mockImplementation((options: { range: { requestDataRange(begin: number, end: number): void } }) => {
    options.range.requestDataRange(0, 1000);
    return { promise: Promise.resolve({ getPage: fixture.page }), destroy: fixture.destroy.mockResolvedValue(undefined) };
  });
  const result = await readFileThumbnail(client, reference, native, new AbortController().signal);
  expect(result).toHaveProperty("canvas"); expect(fixture.page).toHaveBeenCalledWith(1); expect(fixture.render).toHaveBeenCalledWith(expect.objectContaining({ annotationMode: 0 }));
  expect(fixture.pdf).toHaveBeenCalledWith(expect.objectContaining({ disableAutoFetch: true, disableStream: true, enableXfa: false, maxImageSize: 16 * 1024 * 1024, rangeChunkSize: 262144 }));
  expect(fixture.cleanup).toHaveBeenCalledOnce(); expect(fixture.destroy).toHaveBeenCalledOnce(); expect(fixture.workerDestroy).toHaveBeenCalledOnce(); expect(close).toHaveBeenCalledOnce();
});
it("terminates its owned PDF worker even when loading-task teardown rejects", async () => {
  source("application/pdf", new Uint8Array(1000));
  fixture.render.mockReturnValue({ promise: Promise.resolve(), cancel: vi.fn() });
  fixture.page.mockResolvedValue({ getViewport: ({ scale }: { scale: number }) => ({ width: 600 * scale, height: 800 * scale }), render: fixture.render, cleanup: fixture.cleanup });
  fixture.destroy.mockRejectedValue(new Error("Synthetic transport teardown failure"));
  fixture.pdf.mockReturnValue({ promise: Promise.resolve({ getPage: fixture.page }), destroy: fixture.destroy });
  await readFileThumbnail(client, reference, native, new AbortController().signal);
  expect(fixture.destroy).toHaveBeenCalledOnce(); expect(fixture.workerDestroy).toHaveBeenCalledOnce(); expect(close).toHaveBeenCalledOnce();
  await readFileThumbnail(client, reference, native, new AbortController().signal); expect(fixture.workerDestroy).toHaveBeenCalledTimes(2);
});
it("never allocates the PDF worker for a200MiB automatic preview", async () => {
  fixture.preview.mockResolvedValue({ attachment: info("application/pdf", 200 * 1024 * 1024), url, close });
  await expect(readFileThumbnail(client, reference, native, new AbortController().signal)).rejects.toThrow(/limits/); expect(fixture.pdf).not.toHaveBeenCalled(); expect(close).toHaveBeenCalledOnce();
});
it("stops PDF loading before range requests exceed2MiB and retires its worker", async () => {
  const fetcher = source("application/pdf", new Uint8Array(4 * 1024 * 1024));
  fixture.pdf.mockImplementation((options: { range: { requestDataRange(begin: number, end: number): void } }) => {
    queueMicrotask(() => { for (let offset = 0; offset < 9 * 262144; offset += 262144) options.range.requestDataRange(offset, offset + 262144); });
    return { promise: new Promise(() => {}), destroy: fixture.destroy.mockResolvedValue(undefined) };
  });
  await expect(readFileThumbnail(client, reference, native, new AbortController().signal)).rejects.toMatchObject({ name: "AbortError" });
  expect(fetcher.mock.calls.length).toBeLessThanOrEqual(8); expect(fixture.destroy).toHaveBeenCalled(); expect(close).toHaveBeenCalledOnce();
});
it("creates an actual muted video frame through the scoped native URL without playback or a CSP-blocked blob", async () => {
  source("video/mp4", new Uint8Array(64));
  const original = document.createElement.bind(document), pause = vi.fn(), play = vi.fn(); let sourceUrl: string | null = null, muted = false;
  const elements: { createElement(name: string, options?: ElementCreationOptions): HTMLElement } = document;
  vi.spyOn(elements, "createElement").mockImplementation((name: string, options?: ElementCreationOptions) => {
    const element = original(name, options);
    if (name === "video") {
      Object.defineProperties(element, { videoWidth: { value: 640 }, videoHeight: { value: 360 }, duration: { value: 8 }, currentTime: { set: () => queueMicrotask(() => element.dispatchEvent(new Event("seeked"))) }, load: { value: () => { if (element.hasAttribute("src")) { sourceUrl = element.getAttribute("src"); muted = (element as HTMLVideoElement).muted; queueMicrotask(() => element.dispatchEvent(new Event("loadedmetadata"))); } } }, pause: { value: pause }, play: { value: play } });
    }
    return element;
  });
  const result = await readFileThumbnail(client, reference, native, new AbortController().signal);
  expect(result).toHaveProperty("canvas"); expect(play).not.toHaveBeenCalled(); expect(pause).toHaveBeenCalledOnce(); expect(sourceUrl).toBe(url); expect(muted).toBe(true); expect(close).toHaveBeenCalledOnce();
});
it("retires an active thumbnail before foreground work and does not admit queued cards while paused", async () => {
  let started!: () => void, cleanup!: () => void;
  const began = new Promise<void>((resolve) => { started = resolve; }), cleaned = new Promise<void>((resolve) => { cleanup = resolve; });
  const first = queueThumbnail(new AbortController().signal, async (signal) => { started(); await new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true })); await cleaned; });
  await began; const next = vi.fn(async () => {}), second = queueThumbnail(new AbortController().signal, next); const secondResult = second.catch((error: unknown) => error);
  const pause = pauseThumbnailWork(); let ready = false; void pause.ready.then(() => { ready = true; }); await Promise.resolve(); expect(ready).toBe(false);
  cleanup(); await first; await pause.ready; expect(ready).toBe(true); expect(next).not.toHaveBeenCalled(); expect(await secondResult).toMatchObject({ name: "AbortError" });
  pause.release(); await queueThumbnail(new AbortController().signal, next); expect(next).toHaveBeenCalledOnce();
});
