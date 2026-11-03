// @vitest-environment happy-dom
import type { Client } from "@tjommie/client";
import { afterEach, expect, it, vi } from "vitest";
import type { AttachmentNativeApi, AttachmentOpen } from "../../protocol/src/attachments";
import { invalidateThumbnailWork, pauseThumbnailWork, queueThumbnail, readFileThumbnail } from "./file-thumbnails";
import { PreviewRetirementError } from "./attachment-transfer";

const reference = { messageId: "message", attachmentIndex: 0 }, previewId = "624d43d7-1a49-4d79-8d1d-85d846cfbd8e";
const transfer: AttachmentOpen = { transferId: previewId, chunkBytes: 262144, expiresAt: "2026-11-03T12:00:00Z", attachment: { id: previewId, hostId: "storage-host", name: "empty.md", size: 0, contentType: "text/markdown", sha256: "a".repeat(64) } };
afterEach(() => vi.useRealTimers());
it.each(["host", "native"] as const)("awaits a delayed %s open and its actual cleanup before admitting the foreground", async (phase) => {
  let opened!: (value: AttachmentOpen) => void, previewed!: (value: { previewId: string; url: string }) => void, retired!: () => void;
  const pendingOpen = new Promise<AttachmentOpen>((resolve) => { opened = resolve; }), pendingPreview = new Promise<{ previewId: string; url: string }>((resolve) => { previewed = resolve; });
  const closed = new Promise<void>((resolve) => { retired = resolve; });
  const call = vi.fn(async (method: string) => method === "attachments.open" ? phase === "host" ? pendingOpen : transfer : method === "attachments.close" && phase === "host" ? closed : {});
  const client = { call } as unknown as Pick<Client, "call">;
  const native = { preview: vi.fn(() => phase === "native" ? pendingPreview : Promise.resolve({ previewId, url: `app://tjommie/_attachments/${previewId}` })), closePreview: vi.fn(() => closed) } as unknown as AttachmentNativeApi;
  const background = queueThumbnail(new AbortController().signal, (signal) => readFileThumbnail(client, reference, native, signal)); const stopped = background.catch((error: unknown) => error);
  await vi.waitFor(() => expect(phase === "host" ? call : native.preview).toHaveBeenCalled());
  const pause = pauseThumbnailWork(); let admitted = false; void pause.ready.then(() => { admitted = true; });
  await Promise.resolve(); await Promise.resolve(); expect(admitted).toBe(false);
  if (phase === "host") opened(transfer); else previewed({ previewId, url: `app://tjommie/_attachments/${previewId}` });
  await vi.waitFor(() => expect(phase === "host" ? call : native.closePreview).toHaveBeenCalledWith(...(phase === "host" ? ["attachments.close", { transferId: previewId }] : [previewId])));
  expect(admitted).toBe(false); retired(); await pause.ready; expect(admitted).toBe(true); expect(await stopped).toMatchObject({ name: "AbortError" }); pause.release();
});
it("bounds a foreground retirement wait without treating the deadline as permission to open", async () => {
  vi.useFakeTimers(); let cleanup!: () => void, began!: () => void;
  const started = new Promise<void>((resolve) => { began = resolve; }), retired = new Promise<void>((resolve) => { cleanup = resolve; });
  const background = queueThumbnail(new AbortController().signal, async () => { began(); await retired; }); await started;
  const pause = pauseThumbnailWork(), failure = pause.ready.catch((error: unknown) => error), foreground = vi.fn();
  void pause.ready.then(foreground, () => {}); await vi.advanceTimersByTimeAsync(30_001);
  expect(await failure).toBeInstanceOf(Error); expect(foreground).not.toHaveBeenCalled();
  cleanup(); await background; pause.release();
});
it("recovers a failed thumbnail generation only after verified native invalidation", async () => {
  await expect(queueThumbnail(new AbortController().signal, async () => { throw new PreviewRetirementError(); })).rejects.toBeInstanceOf(PreviewRetirementError);
  const blocked = pauseThumbnailWork(); await expect(blocked.ready).rejects.toBeInstanceOf(PreviewRetirementError); blocked.release();
  let cleaned!: () => void; const cleanup = new Promise<void>((resolve) => { cleaned = resolve; }); invalidateThumbnailWork(cleanup);
  const foreground = vi.fn(), fresh = pauseThumbnailWork(); void fresh.ready.then(foreground); await Promise.resolve(); expect(foreground).not.toHaveBeenCalled();
  cleaned(); await fresh.ready; expect(foreground).toHaveBeenCalledOnce(); fresh.release();
  const thumbnail = vi.fn(async () => "fresh"); expect(await queueThumbnail(new AbortController().signal, thumbnail)).toBe("fresh"); expect(thumbnail).toHaveBeenCalledOnce();
});
it("does not mistake failed native invalidation for retirement proof", async () => {
  invalidateThumbnailWork(Promise.reject(new Error("Synthetic native failure")));
  const blocked = pauseThumbnailWork(); await expect(blocked.ready).rejects.toThrow("Synthetic native failure"); blocked.release();
  const next = vi.fn(async () => {}); await expect(queueThumbnail(new AbortController().signal, next)).rejects.toThrow("Synthetic native failure"); expect(next).not.toHaveBeenCalled();
  invalidateThumbnailWork(Promise.resolve()); const retry = pauseThumbnailWork(); await retry.ready; retry.release();
});
it("keeps local decoder teardown serial after native handles are invalidated", async () => {
  let began!: () => void, decoded!: () => void;
  const started = new Promise<void>((resolve) => { began = resolve; }), decoder = new Promise<void>((resolve) => { decoded = resolve; });
  const first = queueThumbnail(new AbortController().signal, async () => { began(); await decoder; }); await started;
  invalidateThumbnailWork(Promise.resolve()); const second = vi.fn(async () => {}), next = queueThumbnail(new AbortController().signal, second);
  await Promise.resolve(); await Promise.resolve(); expect(second).not.toHaveBeenCalled(); decoded(); await first; await next; expect(second).toHaveBeenCalledOnce();
});
