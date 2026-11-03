import { createHash, randomUUID } from "node:crypto";
import { open, rename, rm, type FileHandle } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo } from "../../protocol/src/attachments.ts";

type Save = { owner: string; destination: string; temporary: string; info: AttachmentInfo; file: FileHandle; hash: ReturnType<typeof createHash>; written: number; writing: boolean; finishing: boolean; cancelled: boolean; signal?: AbortSignal; abort: () => void };
type Opening = { owner: string; cancelled: boolean; done: Promise<void>; finish: () => void };
const base64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
export function decodeAttachmentChunk(value: unknown): Buffer {
  if (typeof value !== "string" || !value.length || value.length > 4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3) || !base64.test(value)) throw new Error("Invalid file chunk.");
  const bytes = Buffer.from(value, "base64");
  if (bytes.length > ATTACHMENT_CHUNK_BYTES || bytes.toString("base64") !== value) throw new Error("Invalid file chunk.");
  return bytes;
}

// The caller supplies a native Save As result and trusted renderer identity, never a renderer path.
export class AttachmentSaves {
  #saves = new Map<string, Save>();
  #opening = new Map<string, Opening>();
  #cleanups = new Map<Save, Promise<void>>();
  async start(owner: string, metadata: AttachmentInfo, destination: string, signal?: AbortSignal): Promise<{ saveId: string }> {
    const info = AttachmentInfo.parse(metadata);
    if (!owner || signal?.aborted) throw new Error("The file connection is closed.");
    if (this.#saves.size + this.#opening.size >= 16 || [...this.#saves.values()].filter((save) => save.owner === owner).length + [...this.#opening.values()].filter((value) => value.owner === owner).length >= 2) throw new Error("Two downloads are already active.");
    const id = randomUUID(), temporary = join(dirname(destination), `.${basename(destination)}.${id}.part`);
    let finish = () => {}; const done = new Promise<void>((resolve) => { finish = resolve; }), opening: Opening = { owner, cancelled: false, done, finish };
    this.#opening.set(id, opening);
    let file: FileHandle;
    try { file = await open(temporary, "wx", 0o600); }
    catch (error) { this.#opening.delete(id); opening.finish(); throw error; }
    const save: Save = { owner, destination, temporary, info, file, hash: createHash("sha256"), written: 0, writing: false, finishing: false, cancelled: false, signal, abort: () => { void this.cancelOwner(owner).catch(() => {}); } };
    try {
      this.#saves.set(id, save);
      if (signal?.aborted || opening.cancelled) { await this.#discard(id, save); throw new Error("The file connection is closed."); }
      signal?.addEventListener("abort", save.abort, { once: true }); return { saveId: id };
    } finally { this.#opening.delete(id); opening.finish(); }
  }
  #get(owner: string, id: string): Save {
    const save = this.#saves.get(id);
    if (!save || save.owner !== owner || save.cancelled || save.signal?.aborted) throw new Error("This download is no longer available."); return save;
  }
  async append(owner: string, id: string, offset: number, encoded: unknown): Promise<void> {
    const save = this.#get(owner, id), bytes = decodeAttachmentChunk(encoded);
    if (!Number.isSafeInteger(offset) || offset !== save.written || offset + bytes.length > save.info.size) throw new Error("The download chunk is out of sequence.");
    if (save.writing || save.finishing) throw new Error("A download write is already in progress."); save.writing = true;
    try {
      for (let at = 0; at < bytes.length;) {
        this.#get(owner, id); const result = await save.file.write(bytes, at, bytes.length - at); if (!result.bytesWritten) throw new Error("The download could not be written."); at += result.bytesWritten;
      }
      this.#get(owner, id); save.hash.update(bytes); save.written += bytes.length;
    } catch (error) { await this.#discard(id, save); throw error; }
    finally { save.writing = false; bytes.fill(0); }
  }
  async finish(owner: string, id: string): Promise<{ status: "saved" }> {
    const save = this.#get(owner, id);
    if (save.writing || save.finishing) throw new Error("A download write is already in progress."); save.finishing = true;
    try {
      if (save.written !== save.info.size || save.hash.digest("hex") !== save.info.sha256) throw new Error("The download failed its content check.");
      await save.file.sync(); this.#get(owner, id); await save.file.close(); this.#get(owner, id);
      await rename(save.temporary, save.destination); this.#saves.delete(id); save.signal?.removeEventListener("abort", save.abort);
      return { status: "saved" };
    } catch (error) { await this.#discard(id, save); throw error; }
  }
  #discard(id: string, save: Save): Promise<void> {
    const pending = this.#cleanups.get(save); if (pending) return pending;
    const cleanup = (async () => {
      save.cancelled = true; save.finishing = false; save.signal?.removeEventListener("abort", save.abort);
      await save.file.close().catch(() => {}); await rm(save.temporary, { force: true });
      this.#saves.delete(id);
    })();
    this.#cleanups.set(save, cleanup); void cleanup.finally(() => this.#cleanups.delete(save)).catch(() => {}); return cleanup;
  }
  async cancel(owner: string, id: string): Promise<{ status: "cancelled" }> {
    const save = this.#saves.get(id); if (!save) return { status: "cancelled" };
    if (save.owner !== owner) throw new Error("This download is not authorized.");
    if (save.finishing) throw new Error("This download is finishing.");
    await this.#discard(id, save); return { status: "cancelled" };
  }
  async cancelOwner(owner: string): Promise<void> {
    const opening = [...this.#opening.values()].filter((entry) => entry.owner === owner); for (const entry of opening) entry.cancelled = true;
    await Promise.allSettled([...this.#saves.entries()].filter(([, save]) => save.owner === owner).map(([id, save]) => this.#discard(id, save)).concat(opening.map((entry) => entry.done)));
    await Promise.all([...this.#saves.entries()].filter(([, save]) => save.owner === owner).map(([id, save]) => this.#discard(id, save)).concat([...this.#cleanups].filter(([save]) => save.owner === owner).map(([, cleanup]) => cleanup)));
  }
}
