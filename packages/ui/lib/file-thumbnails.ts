import type { Client } from "@tjommie/client";
import { ATTACHMENT_CHUNK_BYTES, type AttachmentInfo, type AttachmentNativeApi, type AttachmentRef } from "../../protocol/src/attachments";
import { previewAttachment, PreviewRetirementError } from "./attachment-transfer";
import { documentRange, PdfRangeQueue } from "./document-ranges";

export const THUMBNAIL_IMAGE_BYTES = 16 * 1024 * 1024;
export const THUMBNAIL_VIDEO_BYTES = 8 * 1024 * 1024;
export const THUMBNAIL_PIXELS = 16 * 1024 * 1024;
export const THUMBNAIL_PDF_BYTES = 2 * 1024 * 1024;
// The pinned PDF worker reserves a document-length buffer, even when ranges are used.
export const THUMBNAIL_PDF_FILE_BYTES = 16 * 1024 * 1024;
export const THUMBNAIL_TEXT_BYTES = 4096;
const WIDTH = 320, HEIGHT = 180;
export type ThumbnailKind = "image" | "pdf" | "video" | "text" | "audio" | "file";
export type FileThumbnailContent = { canvas: HTMLCanvasElement } | { text: string };
export function thumbnailKind(type: string): ThumbnailKind {
  if (/^image\/(png|jpeg|webp|gif)$/.test(type)) return "image";
  if (type === "application/pdf") return "pdf";
  if (/^video\/(mp4|webm)$/.test(type)) return "video";
  if (/^audio\/(wav|mpeg|mp4|ogg)$/.test(type)) return "audio";
  if (type === "text/markdown" || type === "text/plain") return "text";
  return "file";
}
export function thumbnailEligible(type: string, size: number): boolean {
  if (!Number.isSafeInteger(size) || size < 0) return false;
  switch (thumbnailKind(type)) {
    case "image": return size > 0 && size <= THUMBNAIL_IMAGE_BYTES;
    case "video": return size > 0 && size <= THUMBNAIL_VIDEO_BYTES;
    case "pdf": return size > 0 && size <= THUMBNAIL_PDF_FILE_BYTES;
    case "text": return size <= 200 * 1024 * 1024;
    default: return false;
  }
}

// One background preview at a time; a user-opened viewer or save retires it first.
const newGeneration = () => ({ failed: false, retired: false, listeners: new Set<() => void>() });
let generation = newGeneration(), nativeRetired: Promise<void> = Promise.resolve();
const retiring = new Set<ReturnType<typeof newGeneration>>();
let tail: Promise<void> = Promise.resolve(), running: AbortController | null = null, pauses = 0;
const pauseListeners = new Set<() => void>();
export const thumbnailsPaused = () => pauses > 0;
export function onThumbnailPauseChange(listener: () => void): () => void { pauseListeners.add(listener); return () => { pauseListeners.delete(listener); }; }
/** Only a successful native invalidation proves previous-generation handles retired. */
export function invalidateThumbnailWork(cleanup: Promise<void>): void {
  retiring.add(generation); generation = newGeneration(); running?.abort();
  const previous = [...retiring];
  nativeRetired = cleanup.then(() => { for (const scope of previous) { scope.retired = true; for (const listener of scope.listeners) listener(); scope.listeners.clear(); retiring.delete(scope); } });
  void nativeRetired.catch(() => {});
}
export function queueThumbnail<T>(signal: AbortSignal, work: (signal: AbortSignal, nativeInvalidated: Promise<void>) => Promise<T>): Promise<T> {
  if (pauses) return Promise.reject(new DOMException("Preview paused", "AbortError"));
  const scope = generation, nativeCleanup = nativeRetired;
  const result = tail.then(async () => {
    await nativeCleanup; signal.throwIfAborted(); if (scope !== generation) throw new DOMException("Preview invalidated", "AbortError");
    if (scope.failed) throw new PreviewRetirementError(); if (pauses) throw new DOMException("Preview paused", "AbortError");
    const abort = new AbortController(), cancelled = () => abort.abort(); running = abort;
    signal.addEventListener("abort", cancelled, { once: true });
    const timer = setTimeout(cancelled, 10_000);
    let retired = () => {};
    const invalidated = new Promise<void>((resolve) => { retired = resolve; scope.listeners.add(retired); });
    const working = work(abort.signal, invalidated).catch((error: unknown) => { if (error instanceof PreviewRetirementError && !scope.retired) scope.failed = true; throw error; });
    try { return await working; }
    finally { clearTimeout(timer); scope.listeners.delete(retired); signal.removeEventListener("abort", cancelled); if (running === abort) running = null; }
  });
  tail = result.then(() => {}, () => {}); return result;
}
export function pauseThumbnailWork(signal?: AbortSignal): { ready: Promise<void>; release(): void } {
  pauses++; running?.abort(); for (const listener of pauseListeners) listener();
  let released = false, timer: ReturnType<typeof setTimeout> | undefined;
  const ready = new Promise<void>((resolve, reject) => {
    timer = setTimeout(() => reject(new PreviewRetirementError()), 30_000);
    void Promise.all([tail, nativeRetired]).then(() => { if (generation.failed) reject(new PreviewRetirementError()); else resolve(); }, reject);
  });
  return { ready: (signal ? abortable(ready, signal) : ready).finally(() => clearTimeout(timer)), release() { if (released) return; released = true; pauses--; for (const listener of pauseListeners) listener(); } };
}

function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  signal.throwIfAborted(); let cancel = () => {};
  return new Promise<T>((resolve, reject) => {
    cancel = () => reject(new DOMException("Preview cancelled", "AbortError"));
    signal.addEventListener("abort", cancel, { once: true }); void promise.then(resolve, reject);
  }).finally(() => signal.removeEventListener("abort", cancel));
}
function dimensions(width: number, height: number): { width: number; height: number } {
  if (![width, height].every((value) => Number.isSafeInteger(value) && value > 0) || width * height > THUMBNAIL_PIXELS) throw new RangeError("Image exceeds thumbnail dimensions.");
  return { width, height };
}
/** Inspect encoded dimensions before asking the browser to decode untrusted raster data. */
export function rasterDimensions(bytes: Uint8Array, type: string): { width: number; height: number } {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength), ascii = (begin: number, end: number) => String.fromCharCode(...bytes.subarray(begin, end));
  if (type === "image/png" && bytes.length >= 24 && bytes[0] === 137 && ascii(1, 8) === "PNG\r\n\x1a\n" && ascii(12, 16) === "IHDR") return dimensions(view.getUint32(16), view.getUint32(20));
  if (type === "image/gif" && bytes.length >= 10 && /^GIF8[79]a$/.test(ascii(0, 6))) return dimensions(view.getUint16(6, true), view.getUint16(8, true));
  if (type === "image/webp" && bytes.length >= 30 && ascii(0, 4) === "RIFF" && ascii(8, 12) === "WEBP") {
    const kind = ascii(12, 16), uint24 = (offset: number) => bytes[offset] + bytes[offset + 1] * 256 + bytes[offset + 2] * 65536;
    if (kind === "VP8X") return dimensions(uint24(24) + 1, uint24(27) + 1);
    if (kind === "VP8 " && bytes[23] === 157 && bytes[24] === 1 && bytes[25] === 42) return dimensions(view.getUint16(26, true) & 16383, view.getUint16(28, true) & 16383);
    if (kind === "VP8L" && bytes[20] === 47) { const bits = view.getUint32(21, true); return dimensions((bits & 16383) + 1, ((bits >>> 14) & 16383) + 1); }
  }
  if (type === "image/jpeg" && bytes.length >= 4 && bytes[0] === 255 && bytes[1] === 216) {
    let at = 2;
    while (at + 3 < bytes.length) {
      if (bytes[at++] !== 255) break;
      while (bytes[at] === 255) at++;
      const marker = bytes[at++]; if (marker === 217 || marker === 218) break;
      if (marker === 1 || marker >= 208 && marker <= 215) continue;
      if (at + 2 > bytes.length) break;
      const length = view.getUint16(at); if (length < 2 || at + length > bytes.length) break;
      if ([192, 193, 194, 195, 197, 198, 199, 201, 202, 203, 205, 206, 207].includes(marker) && length >= 7) return dimensions(view.getUint16(at + 5), view.getUint16(at + 3));
      at += length;
    }
  }
  throw new Error("Unsupported raster header.");
}
function canvas(): HTMLCanvasElement { const value = document.createElement("canvas"); value.width = WIDTH; value.height = HEIGHT; return value; }
function draw(source: CanvasImageSource, width: number, height: number): HTMLCanvasElement {
  const target = canvas(), context = target.getContext("2d"); if (!context) throw new Error("Canvas unavailable.");
  const scale = Math.min(WIDTH / width, HEIGHT / height), w = width * scale, h = height * scale;
  context.drawImage(source, (WIDTH - w) / 2, (HEIGHT - h) / 2, w, h); return target;
}
async function bytesFor(url: string, size: number, signal: AbortSignal): Promise<Uint8Array> {
  const bytes = new Uint8Array(size);
  for (let offset = 0; offset < size; offset += ATTACHMENT_CHUNK_BYTES) bytes.set(await documentRange(url, size, offset, Math.min(size, offset + ATTACHMENT_CHUNK_BYTES), signal), offset);
  return bytes;
}
async function image(url: string, info: AttachmentInfo, signal: AbortSignal): Promise<HTMLCanvasElement> {
  const bytes = await bytesFor(url, info.size, signal), size = rasterDimensions(bytes, info.contentType), scale = Math.min(1, WIDTH / size.width, HEIGHT / size.height);
  const pending = createImageBitmap(new Blob([bytes.buffer as ArrayBuffer], { type: info.contentType }), { resizeWidth: Math.max(1, Math.round(size.width * scale)), resizeHeight: Math.max(1, Math.round(size.height * scale)) });
  let bitmap: ImageBitmap;
  try { bitmap = await abortable(pending, signal); } catch (error) { await pending.then((late) => late.close(), () => {}); throw error; }
  try { signal.throwIfAborted(); return draw(bitmap, bitmap.width, bitmap.height); } finally { bitmap.close(); }
}
function mediaEvent(video: HTMLVideoElement, event: string, signal: AbortSignal, action: () => void): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const cleanup = () => { video.removeEventListener(event, done); video.removeEventListener("error", failed); signal.removeEventListener("abort", cancelled); };
    const done = () => { cleanup(); resolve(); }, failed = () => { cleanup(); reject(new Error("Video thumbnail unavailable.")); }, cancelled = () => { cleanup(); reject(new DOMException("Preview cancelled", "AbortError")); };
    video.addEventListener(event, done, { once: true }); video.addEventListener("error", failed, { once: true }); signal.addEventListener("abort", cancelled, { once: true });
    if (signal.aborted) cancelled(); else { try { action(); } catch { failed(); } }
  });
}
async function video(url: string, signal: AbortSignal): Promise<HTMLCanvasElement> {
  const element = document.createElement("video");
  element.muted = true; element.playsInline = true; element.preload = "metadata";
  try {
    await mediaEvent(element, "loadedmetadata", signal, () => { element.src = url; element.load(); });
    dimensions(element.videoWidth, element.videoHeight);
    if (Number.isFinite(element.duration) && element.duration > 0) await mediaEvent(element, "seeked", signal, () => { element.currentTime = Math.min(1, element.duration / 4); });
    else if (element.readyState < 2) await mediaEvent(element, "loadeddata", signal, () => {});
    signal.throwIfAborted(); return draw(element, element.videoWidth, element.videoHeight);
  } finally { element.pause(); element.removeAttribute("src"); element.load(); }
}
type PdfPage = { getViewport(options: { scale: number }): { width: number; height: number }; render(options: Record<string, unknown>): { promise: Promise<void>; cancel(): void }; cleanup(): boolean };
type PdfLoading = { promise: Promise<{ getPage(page: number): Promise<PdfPage> }>; onPassword?: () => void; destroy(): Promise<void> };
type PdfRange = { onDataRange(begin: number, bytes: ArrayBuffer): void; requestDataRange(begin: number, end: number): void; abort(): void };
type PdfWorker = { destroy(): void };
type PdfApi = { GlobalWorkerOptions: { workerSrc: string }; PDFWorker: new () => PdfWorker; PDFDataRangeTransport: new (size: number, initial: Uint8Array, done: boolean) => PdfRange; getDocument(options: Record<string, unknown>): PdfLoading };
async function pdf(url: string, size: number, signal: AbortSignal): Promise<HTMLCanvasElement> {
  const api = await abortable(import(/* webpackIgnore: true */ new URL("/pdfjs/build/pdf.mjs", window.location.href).href) as Promise<PdfApi>, signal);
  api.GlobalWorkerOptions.workerSrc = new URL("/pdfjs/build/pdf.worker.mjs", window.location.href).href;
  let loading: PdfLoading | undefined, worker: PdfWorker | undefined, page: PdfPage | undefined, render: ReturnType<PdfPage["render"]> | undefined, destroying: Promise<void> | undefined, requested = 0, stopped = false;
  const live = new AbortController();
  const destroy = () => loading ? destroying ??= Promise.resolve().then(() => loading!.destroy()) : Promise.resolve();
  const failed = () => { if (stopped) return; stopped = true; live.abort(); render?.cancel(); void destroy().catch(() => {}); };
  signal.addEventListener("abort", failed, { once: true });
  class Ranges extends api.PDFDataRangeTransport {
    #queue = new PdfRangeQueue({ url, size, signal: live.signal, deliver: (begin, bytes) => this.onDataRange(begin, bytes), fail: failed });
    requestDataRange(begin: number, end: number) {
      if (stopped || signal.aborted) return;
      if (!Number.isSafeInteger(end - begin) || end <= begin || requested + end - begin > THUMBNAIL_PDF_BYTES) { failed(); return; }
      requested += end - begin; this.#queue.request(begin, end);
    }
    abort() { failed(); }
  }
  try {
    worker = new api.PDFWorker();
    loading = api.getDocument({ worker, range: new Ranges(size, new Uint8Array(0), true), length: size, rangeChunkSize: ATTACHMENT_CHUNK_BYTES, disableStream: true, disableAutoFetch: true, enableXfa: false, disableWorkerRendering: true, maxImageSize: THUMBNAIL_PIXELS, canvasMaxAreaInBytes: WIDTH * HEIGHT * 4, standardFontDataUrl: "/pdfjs/standard_fonts/", cMapUrl: "/pdfjs/cmaps/", wasmUrl: "/pdfjs/wasm/" });
    loading.onPassword = failed;
    const document = await abortable(loading.promise, live.signal); page = await abortable(document.getPage(1), live.signal);
    const viewport = page.getViewport({ scale: 1 }), scale = Math.min(WIDTH / viewport.width, HEIGHT / viewport.height);
    if (![viewport.width, viewport.height, scale].every((value) => Number.isFinite(value) && value > 0)) throw new Error("Invalid PDF page.");
    const target = canvas(); target.width = Math.max(1, Math.floor(viewport.width * scale)); target.height = Math.max(1, Math.floor(viewport.height * scale));
    render = page.render({ canvasContext: target.getContext("2d"), viewport: page.getViewport({ scale }), annotationMode: 0 }); await abortable(render.promise, live.signal); signal.throwIfAborted(); return target;
  } finally {
    signal.removeEventListener("abort", failed); stopped = true; live.abort();
    try { render?.cancel(); page?.cleanup(); }
    finally { try { await destroy().catch(() => {}); } finally { worker?.destroy(); } }
  }
}

export async function readFileThumbnail(client: Pick<Client, "call">, reference: AttachmentRef, native: AttachmentNativeApi, signal: AbortSignal, nativeInvalidated?: Promise<void>): Promise<FileThumbnailContent> {
  let preview: Awaited<ReturnType<typeof previewAttachment>> | undefined;
  const retiring: Promise<void>[] = [];
  try {
    preview = await previewAttachment(client, reference, native, signal, undefined, (cleanup) => { retiring.push(cleanup); });
    const info = preview.attachment; if (!thumbnailEligible(info.contentType, info.size)) throw new RangeError("File exceeds thumbnail limits.");
    switch (thumbnailKind(info.contentType)) {
      case "image": return { canvas: await image(preview.url, info, signal) };
      case "video": return { canvas: await video(preview.url, signal) };
      case "pdf": return { canvas: await pdf(preview.url, info.size, signal) };
      case "text": {
        const bytes = info.size ? await documentRange(preview.url, info.size, 0, Math.min(info.size, THUMBNAIL_TEXT_BYTES), signal) : new Uint8Array();
        const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes, { stream: bytes.length < info.size });
        if (/[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(text)) throw new Error("Not a text thumbnail.");
        return { text: text.slice(0, 600) };
      }
      default: throw new Error("No thumbnail for this file type.");
    }
  } finally {
    const cleanup = (async () => { await preview?.close(); await Promise.all(retiring); })();
    try { await (nativeInvalidated ? Promise.race([cleanup, nativeInvalidated]) : cleanup); }
    catch { throw new PreviewRetirementError(); }
  }
}
