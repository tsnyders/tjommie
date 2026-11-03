import { ClientConnectionError, ClientRpcError, type Client } from "@tjommie/client";
import { ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentInfo, AttachmentOpen, type AttachmentNativeApi, type AttachmentRef } from "../../protocol/src/attachments";

export function attachmentError(error: unknown): string {
  if (error instanceof ClientConnectionError) return "The file host is offline. Reconnect and try again.";
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : null;
  switch (code) {
    case "forbidden": return "This device no longer has permission to read the file.";
    case "unavailable": return error instanceof ClientRpcError ? "This shared file is unavailable; it may have been removed." : "The file or save destination is unavailable.";
    case "corrupt": return "The file failed its content check. Open it again or request a fresh share.";
    case "expired": return "This file transfer expired. Open the file again.";
    case "busy": return "Two file transfers are already active. Close one and try again.";
    case "cleanup_failed": return "The save provider kept an incomplete file. Remove it from the selected destination.";
    default: return "The file could not be opened or saved. Check the connection and try again.";
  }
}

export function attachmentBytes(chunk: AttachmentChunk, transfer: AttachmentOpen, offset: number, length: number): Uint8Array {
  AttachmentChunk.parse(chunk);
  const expected = Math.min(length, transfer.attachment.size - offset);
  if (chunk.transferId !== transfer.transferId || chunk.revision !== transfer.attachment.sha256 || chunk.offset !== offset || chunk.eof !== (offset + expected === transfer.attachment.size)) throw new Error("The file host returned an invalid chunk.");
  const binary = atob(chunk.bytes), bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  if (bytes.length !== expected || bytes.length > ATTACHMENT_CHUNK_BYTES || btoa(binary) !== chunk.bytes) throw new Error("The file host returned invalid bytes."); return bytes;
}
function untilAbort<T>(promise: Promise<T>, signal: AbortSignal, deadline = true): Promise<T> {
  if (signal.aborted) return Promise.reject(new DOMException("Cancelled", "AbortError"));
  let timer: ReturnType<typeof setTimeout> | undefined;
  let abort = () => {};
  return new Promise<T>((accept, reject) => {
    abort = () => reject(new DOMException("Cancelled", "AbortError"));
    if (deadline) timer = setTimeout(() => reject(new Error("The file host did not answer.")), 30000);
    signal.addEventListener("abort", abort, { once: true });
    void promise.then(accept, reject);
  }).finally(() => { clearTimeout(timer); signal.removeEventListener("abort", abort); });
}

export class AttachmentTransfer {
  readonly transfer: AttachmentOpen;
  #client: Pick<Client, "call">;
  #signal: AbortSignal;
  #closed = false;
  #reading = false;
  #strictRetirement: boolean;
  private constructor(client: Pick<Client, "call">, transfer: AttachmentOpen, signal: AbortSignal, strictRetirement = false) { this.#client = client; this.transfer = transfer; this.#signal = signal; this.#strictRetirement = strictRetirement; }
  static async open(client: Pick<Client, "call">, ref: AttachmentRef, signal: AbortSignal, retiring?: (cleanup: Promise<void>) => void): Promise<AttachmentTransfer> {
    const pending = client.call("attachments.open", ref), result = await untilAbort(pending, signal).catch((error: unknown) => {
      const cleanup = pending.then((late) => client.call("attachments.close", { transferId: late.transferId }), () => {}).then(() => {});
      retiring?.(cleanup); void cleanup.catch(() => {}); throw error;
    });
    if (signal.aborted) { const cleanup = client.call("attachments.close", { transferId: result.transferId }).then(() => {}); retiring?.(cleanup); await cleanup.catch(() => {}); throw new DOMException("Cancelled", "AbortError"); }
    return new AttachmentTransfer(client, AttachmentOpen.parse(result), signal, retiring !== undefined);
  }
  get info(): AttachmentInfo { return this.transfer.attachment; }
  async read(offset: number, length: number): Promise<AttachmentChunk> {
    if (this.#closed || this.#signal.aborted) throw new DOMException("Cancelled", "AbortError");
    if (this.#reading || !Number.isSafeInteger(offset) || offset < 0 || offset > this.info.size || !Number.isSafeInteger(length) || length < 1 || length > ATTACHMENT_CHUNK_BYTES) throw new Error("Invalid or overlapping file read.");
    this.#reading = true;
    try { const chunk = await untilAbort(this.#client.call("attachments.read", { transferId: this.transfer.transferId, offset, length }), this.#signal); if (this.#closed || this.#signal.aborted) throw new DOMException("Cancelled", "AbortError"); attachmentBytes(chunk, this.transfer, offset, length); return chunk; }
    finally { this.#reading = false; }
  }
  async close(): Promise<void> { if (this.#closed) return; this.#closed = true; const cleanup = this.#client.call("attachments.close", { transferId: this.transfer.transferId }); if (this.#strictRetirement) await cleanup; else await cleanup.catch(() => {}); }
}

export type AttachmentDownloadOptions = {
  reacquire?(signal: AbortSignal): Promise<Pick<Client, "call">>;
  transferring?(client: Pick<Client, "call">): void;
};
export async function downloadAttachment(client: Pick<Client, "call">, ref: AttachmentRef, native: AttachmentNativeApi, signal: AbortSignal, progress: (written: number, total: number) => void, options: AttachmentDownloadOptions = {}): Promise<"saved" | "cancelled"> {
  const info = AttachmentInfo.parse(await untilAbort(client.call("attachments.metadata", ref), signal));
  let transfer: AttachmentTransfer | null = null, saveId: string | null = null, published = false;
  const abort = () => { if (saveId) void native.cancelSave(saveId).catch(() => {}); void transfer?.close(); };
  signal.addEventListener("abort", abort, { once: true });
  try {
    const starting = native.startSave(info);
    const save = await untilAbort(starting, signal, false).catch((error: unknown) => { void starting.then((late) => { if (late) return native.cancelSave(late.saveId); }).catch(() => {}); throw error; });
    if (!save) return "cancelled"; saveId = save.saveId;
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    const current = options.reacquire ? await untilAbort(options.reacquire(signal), signal, false) : client;
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    options.transferring?.(current); // A disconnect from this point must never rebind an active transfer.
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    transfer = await AttachmentTransfer.open(current, ref, signal);
    if ((["id", "hostId", "name", "size", "contentType", "sha256"] as const).some((key) => info[key] !== transfer!.info[key])) throw new Error("The shared file changed while choosing a destination.");
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    if (native.activateSave) await untilAbort(native.activateSave(saveId), signal);
    for (let offset = 0; offset < transfer.info.size;) {
      const chunk = await transfer.read(offset, ATTACHMENT_CHUNK_BYTES);
      if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
      await untilAbort(native.appendSave(saveId, offset, chunk.bytes), signal); offset += Math.min(ATTACHMENT_CHUNK_BYTES, transfer.info.size - offset); progress(offset, transfer.info.size);
    }
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    const result = await untilAbort(native.finishSave(saveId), signal, false); published = result.status === "saved"; return result.status;
  } finally {
    signal.removeEventListener("abort", abort);
    let cleanup: unknown; try { if (saveId) await native.cancelSave(saveId); } catch (error) { cleanup = error; }
    await transfer?.close(); if (cleanup && !published) throw cleanup;
  }
}

export class PreviewRetirementError extends Error { constructor() { super("The previous file preview could not close. Reconnect and try again."); } }
export async function previewAttachment(client: Pick<Client, "call">, ref: AttachmentRef, native: AttachmentNativeApi, signal: AbortSignal, onError?: (error: unknown) => void, retiring?: (cleanup: Promise<void>) => void): Promise<{ attachment: AttachmentInfo; url: string; close: () => Promise<void> }> {
  const transfer = await AttachmentTransfer.open(client, ref, signal, retiring); let previewId: string | null = null, remove = () => {}, closing: Promise<void> | undefined;
  const abort = () => { void close().catch(() => {}); };
  function close(): Promise<void> {
    if (closing) return closing;
    signal.removeEventListener("abort", abort); remove();
    return closing = (async () => {
      let failure: unknown;
      try { if (previewId) await native.closePreview(previewId); } catch (error) { failure = error; }
      try { await transfer.close(); } catch (error) { failure ??= error; }
      if (failure) throw failure;
    })();
  }
  signal.addEventListener("abort", abort, { once: true });
  try {
    const opening = native.preview(transfer.info);
    const preview = await untilAbort(opening, signal).catch((error: unknown) => {
      const cleanup = opening.then((late) => native.closePreview(late.previewId), () => {}); retiring?.(cleanup); void cleanup.catch(() => {}); throw error;
    });
    if (signal.aborted) { const cleanup = native.closePreview(preview.previewId); retiring?.(cleanup); await cleanup.catch(() => {}); throw new DOMException("Cancelled", "AbortError"); } previewId = preview.previewId;
    let previous = Promise.resolve(), queued = 0;
    remove = native.onRead((request) => {
      if (request.previewId !== previewId) return;
      if (queued >= 2) { void native.respond(request.previewId, request.requestId, null).catch(() => {}); return; } queued++;
      previous = previous.then(() => transfer.read(request.offset, request.length)).then((chunk) => native.respond(request.previewId, request.requestId, chunk.bytes), (error: unknown) => { if (!signal.aborted) onError?.(error); return native.respond(request.previewId, request.requestId, null); }).catch(() => {}).finally(() => { queued--; });
    });
    if (signal.aborted) throw new DOMException("Cancelled", "AbortError");
    const url = new URL(preview.url);
    if (!(url.protocol === "app:" && url.host === "tjommie" || url.protocol === "https:" && url.host === "localhost") || url.username || url.password || url.search || url.hash || !/^\/_attachments\/[0-9a-f-]{36}$/.test(url.pathname) || url.pathname !== `/_attachments/${preview.previewId}`) throw new Error("The device returned an invalid preview handle.");
    return { attachment: transfer.info, url: preview.url, close };
  } catch (error) { await close(); throw error; }
}
