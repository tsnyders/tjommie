import { ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES } from "../../protocol/src/attachments";

export async function documentRange(url: string, size: number, begin: number, end: number, signal: AbortSignal): Promise<Uint8Array> {
  signal.throwIfAborted();
  if (![size, begin, end].every(Number.isSafeInteger) || size > MAX_ATTACHMENT_BYTES || begin < 0 || end <= begin || end > size || end - begin > ATTACHMENT_CHUNK_BYTES) throw new RangeError("Invalid document byte range.");
  const response = await fetch(url, { headers: { Range: `bytes=${begin}-${end - 1}` }, signal });
  if (response.status !== 206 || response.headers.get("content-range") !== `bytes ${begin}-${end - 1}/${size}` || Number(response.headers.get("content-length")) !== end - begin) {
    await response.body?.cancel().catch(() => {}); throw new Error("The shared document is no longer available.");
  }
  const bytes = new Uint8Array(await response.arrayBuffer()); signal.throwIfAborted();
  if (bytes.byteLength !== end - begin) throw new Error("The document content is incomplete.");
  return bytes;
}

/** The patched pinned PDF worker requests individual chunks, never a full-document renderer buffer. */
export class PdfRangeQueue {
  private tail = Promise.resolve();
  private readonly pending = new Map<number, number>();
  private failed = false;
  constructor(privateOptions: { url: string; size: number; signal: AbortSignal; deliver(begin: number, bytes: ArrayBuffer): void; fail(error: unknown): void }) { this.options = privateOptions; }
  private readonly options;
  request(begin: number, end: number): void {
    const { size, signal } = this.options;
    if (signal.aborted || this.failed) return;
    const overlaps = [...this.pending].some(([first, last]) => begin < last && end > first);
    if (![begin, end].every(Number.isSafeInteger) || begin < 0 || begin % ATTACHMENT_CHUNK_BYTES !== 0 || end !== Math.min(size, begin + ATTACHMENT_CHUNK_BYTES) || end <= begin || overlaps || this.pending.size >= Math.ceil(size / ATTACHMENT_CHUNK_BYTES)) {
      this.failed = true; this.options.fail(new Error("The local PDF reader requested an invalid or overlapping range.")); return;
    }
    this.pending.set(begin, end);
    this.tail = this.tail.then(async () => {
      if (signal.aborted || this.failed) return;
      const bytes = await documentRange(this.options.url, size, begin, end, signal);
      if (!signal.aborted && !this.failed) this.options.deliver(begin, bytes.buffer as ArrayBuffer);
    }).catch((error: unknown) => { if (!signal.aborted && !this.failed) { this.failed = true; this.options.fail(error); } }).finally(() => { this.pending.delete(begin); });
  }
  settled(): Promise<void> { return this.tail; }
}
