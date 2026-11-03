import { createHash } from "node:crypto";
import { closeSync, constants, existsSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeSync, type Stats } from "node:fs";
import { open, type FileHandle } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo, AttachmentRef, MAX_ATTACHMENT_BYTES, type AttachmentChunk, type AttachmentOpen } from "../../protocol/src/attachments.ts";
import { newId, nowIso, transaction, type Db } from "./db.ts";
import type { RequestContext } from "./request-context.ts";
import { RpcError } from "./server.ts";

const LEASE_MS = 5 * 60 * 1000;
type Row = { id: string; host_id: string; storage_name: string; original_name: string; byte_length: number; content_type: string; sha256: string };
type Lease = { id: string; context: RequestContext; owner: string; ref: AttachmentRef; info: AttachmentInfo; file: Promise<FileHandle>; fd: FileHandle | null; stats: Stats | null; reading: boolean; verified: boolean; closed: boolean; closing: Promise<void> | null; expires: number; timer: ReturnType<typeof setTimeout> | null; abort: () => void };
const within = (base: string, file: string) => { const rel = relative(base, file); return rel !== "" && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel); };
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const sameFile = (a: Stats, b: Stats) => a.dev === b.dev && a.ino === b.ino && a.size === b.size && a.mtimeMs === b.mtimeMs && a.ctimeMs === b.ctimeMs;

export function attachmentContentType(name: string, bytes: Uint8Array): string {
  const data = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength), ascii = data.toString("latin1");
  if (data.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) return "image/png";
  if (data[0] === 255 && data[1] === 216 && data[2] === 255) return "image/jpeg";
  if (/^GIF8[79]a/.test(ascii)) return "image/gif";
  if (ascii.startsWith("RIFF") && ascii.slice(8, 12) === "WEBP") return "image/webp";
  if (ascii.startsWith("RIFF") && ascii.slice(8, 12) === "WAVE") return "audio/wav";
  if (ascii.startsWith("%PDF-")) return "application/pdf";
  if (ascii.startsWith("ID3") || data[0] === 255 && [251, 243, 242].includes(data[1])) return "audio/mpeg";
  if (ascii.slice(4, 8) === "ftyp") return /\.m4a$/i.test(name) ? "audio/mp4" : "video/mp4";
  if (data.subarray(0, 4).equals(Buffer.from([26, 69, 223, 163])) && ascii.includes("webm")) return "video/webm";
  if (/\.(md|markdown)$/i.test(name) && !data.includes(0)) {
    try { new TextDecoder("utf-8", { fatal: true }).decode(data, { stream: true }); return "text/markdown"; } catch { /* Binary data keeps the download fallback. */ }
  }
  return "application/octet-stream";
}

export class Attachments {
  #db: Db;
  #hostId: string;
  #staging: string;
  #root: string;
  #leases = new Map<string, Lease>();
  #closed = false;
  constructor(options: { db: Db; root: string; hostId: string }) {
    if (!options.hostId) throw new Error("Attachments need a host identity.");
    this.#db = options.db; this.#hostId = options.hostId;
    this.#staging = resolve(options.root, "shared-files");
    const parent = lstatSync(this.#staging);
    if (!parent.isDirectory() || parent.isSymbolicLink()) throw new Error("The protected shared-files folder is unavailable.");
    this.#staging = realpathSync.native(this.#staging); this.#root = join(this.#staging, "attachments");
    if (!existsSync(this.#root)) mkdirSync(this.#root, { mode: 0o700 });
    this.#checkRoot();
  }
  #checkRoot(): void {
    try {
      const root = lstatSync(this.#root), staging = lstatSync(this.#staging);
      if (root.isDirectory() && !root.isSymbolicLink() && staging.isDirectory() && !staging.isSymbolicLink() && realpathSync.native(this.#root) === this.#root) return;
    } catch { /* Missing and replaced roots have the same safe public error. */ }
    throw new RpcError("unavailable", "Shared files are unavailable.");
  }
  #safeFile(base: string, candidate: string): string | null {
    const file = resolve(candidate);
    try {
      const canonical = realpathSync.native(file);
      if (!within(realpathSync.native(base), canonical)) return null;
      for (let cursor = file;; cursor = dirname(cursor)) {
        if (lstatSync(cursor).isSymbolicLink()) return null;
        if (cursor === dirname(cursor)) break;
      }
      return lstatSync(file).isFile() ? canonical : null;
    } catch { return null; }
  }
  #snapshotPath(candidate: string): string | null {
    const file = this.#safeFile(this.#staging, candidate);
    return file && dirname(dirname(file)) === this.#staging && basename(dirname(file)).startsWith("share-") ? file : null;
  }
  #legacyPath(candidate: string): string | null {
    const staged = this.#snapshotPath(candidate); if (staged) return staged;
    const bots = this.#db.prepare("SELECT home_folder FROM bots").all() as { home_folder: string }[];
    for (const bot of bots) {
      const inbox = resolve(bot.home_folder, "inbox"), file = this.#safeFile(inbox, candidate);
      if (file && dirname(file) === inbox) return file;
    }
    return null;
  }
  registerSnapshot(path: string, name: string): AttachmentInfo {
    this.#checkRoot(); const safe = this.#snapshotPath(path);
    if (!safe) throw new RpcError("bad_params", "Only a protected shared snapshot can be registered.");
    return this.#copy(safe, name);
  }
  #copy(source: string, name: string): AttachmentInfo {
    const id = newId(), storage = `${id}.bin`, destination = join(this.#root, storage), temporary = `${destination}.part`;
    let input: number;
    try { input = openSync(source, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0)); }
    catch { throw new RpcError("unavailable", "The shared snapshot could not be opened."); }
    let output: number | null = null;
    try {
      const initial = fstatSync(input);
      if (!initial.isFile() || initial.size > MAX_ATTACHMENT_BYTES || this.#legacyPath(source) !== source || !sameFile(initial, statSync(source))) throw new RpcError("bad_params", "The shared snapshot is invalid or too large.");
      output = openSync(temporary, "wx", 0o600);
      this.#checkRoot();
      const hash = createHash("sha256"), buffer = Buffer.alloc(64 * 1024); let size = 0, sample = Buffer.alloc(0);
      for (;;) {
        const count = readSync(input, buffer); if (!count) break;
        if (size === 0) sample = Buffer.from(buffer.subarray(0, Math.min(count, 8192)));
        size += count; if (size > MAX_ATTACHMENT_BYTES) throw new RpcError("bad_params", "The file exceeds the 200 MB sharing limit.");
        hash.update(buffer.subarray(0, count));
        for (let offset = 0; offset < count;) { const written = writeSync(output, buffer, offset, count - offset); if (!written) throw new RpcError("unavailable", "The shared file could not be written."); offset += written; }
      }
      if (size !== initial.size || this.#legacyPath(source) !== source || !sameFile(initial, fstatSync(input)) || !sameFile(initial, statSync(source))) throw new RpcError("corrupt", "The shared snapshot changed while being registered.");
      fsyncSync(output); closeSync(output); output = null;
      const info = AttachmentInfo.parse({ id, hostId: this.#hostId, name: basename(name), size, contentType: attachmentContentType(name, sample), sha256: hash.digest("hex") });
      renameSync(temporary, destination);
      this.#db.prepare("INSERT INTO attachments(id,host_id,storage_name,original_name,byte_length,content_type,sha256,created_at) VALUES(?,?,?,?,?,?,?,?)").run(id, info.hostId, storage, info.name, info.size, info.contentType, info.sha256, nowIso());
      return info;
    } catch (error) {
      if (output !== null) closeSync(output);
      try { rmSync(temporary, { force: true }); rmSync(destination, { force: true }); }
      catch { throw new RpcError("unavailable", "The shared file could not be published; temporary cleanup will retry later."); }
      if (record(error) && typeof error.code === "string" && ["EACCES", "ENOENT", "ENOTDIR", "ELOOP", "EPERM", "EIO", "ENOSPC", "EBUSY", "EXDEV"].includes(error.code)) throw new RpcError("unavailable", "The shared file could not be published.");
      throw error;
    } finally { closeSync(input); }
  }
  link(messageId: string, index: number, attachmentId: string): void {
    AttachmentRef.parse({ messageId, attachmentIndex: index, attachmentId });
    if (!this.#db.prepare("SELECT 1 FROM attachments WHERE id=? AND host_id=?").get(attachmentId, this.#hostId)) throw new RpcError("unavailable", "This shared file is unavailable.");
    this.#db.prepare("INSERT INTO message_attachments(message_id,attachment_index,attachment_id) VALUES(?,?,?) ON CONFLICT(message_id,attachment_index) DO UPDATE SET attachment_id=excluded.attachment_id").run(messageId, index, attachmentId);
  }
  #backfillMessage(messageId: string): { registered: number; unavailable: number } {
    const row = this.#db.prepare("SELECT kind,data FROM messages WHERE id=?").get(messageId) as { kind: string; data: string | null } | undefined;
    const result = { registered: 0, unavailable: 0 }; if (!row?.data) return result;
    let data: unknown; try { data = JSON.parse(row.data); } catch { return result; }
    if (!record(data)) return result;
    const files: unknown[] = row.kind === "file" ? [data] : row.kind === "text" && Array.isArray(data.files) ? data.files : [];
    for (const [index, file] of files.entries()) {
      if (!record(file) || typeof file.path !== "string" || typeof file.name !== "string") continue;
      if (this.#db.prepare("SELECT 1 FROM message_attachments WHERE message_id=? AND attachment_index=?").get(messageId, index)) continue;
      const source = this.#legacyPath(file.path);
      if (!source) { result.unavailable++; continue; }
      const size = statSync(source).size;
      if (size > MAX_ATTACHMENT_BYTES || row.kind === "file" && (typeof file.size !== "number" || file.size !== size)) { result.unavailable++; continue; }
      let info: AttachmentInfo;
      try { info = this.#copy(source, file.name); }
      catch (error) { if (error instanceof RpcError && ["bad_params", "corrupt", "unavailable"].includes(error.code)) { result.unavailable++; continue; } throw error; }
      transaction(this.#db, () => this.link(messageId, index, info.id)); result.registered++;
    }
    return result;
  }
  backfillLegacy(): { registered: number; unavailable: number } {
    this.#checkRoot(); const result = { registered: 0, unavailable: 0 };
    const messages = this.#db.prepare("SELECT id FROM messages WHERE kind IN ('file','text') AND data IS NOT NULL").all() as { id: string }[];
    for (const message of messages) { const next = this.#backfillMessage(message.id); result.registered += next.registered; result.unavailable += next.unavailable; }
    return result;
  }
  #context(context?: RequestContext): RequestContext {
    if (this.#closed || !context || context.signal.aborted || !context.connectionId) throw new RpcError("forbidden", "This file connection is no longer authorized.");
    if (context.principal.kind === "device" && !this.#db.prepare("SELECT 1 FROM devices WHERE id=? AND revoked_at IS NULL").get(context.principal.deviceId)) throw new RpcError("forbidden", "This device is no longer authorized.");
    return context;
  }
  #row(ref: AttachmentRef): Row {
    let row = this.#db.prepare("SELECT a.* FROM attachments a JOIN message_attachments m ON m.attachment_id=a.id JOIN messages msg ON msg.id=m.message_id WHERE m.message_id=? AND m.attachment_index=?").get(ref.messageId, ref.attachmentIndex) as Row | undefined;
    if (!row) { this.#backfillMessage(ref.messageId); row = this.#db.prepare("SELECT a.* FROM attachments a JOIN message_attachments m ON m.attachment_id=a.id WHERE m.message_id=? AND m.attachment_index=?").get(ref.messageId, ref.attachmentIndex) as Row | undefined; }
    if (!row || row.host_id !== this.#hostId || ref.attachmentId && row.id !== ref.attachmentId) throw new RpcError("unavailable", "This shared file is unavailable.");
    return row;
  }
  #info(row: Row): AttachmentInfo {
    try { return AttachmentInfo.parse({ id: row.id, hostId: row.host_id, name: row.original_name, size: row.byte_length, contentType: row.content_type, sha256: row.sha256 }); }
    catch { throw new RpcError("corrupt", "The shared file record is invalid."); }
  }
  #path(row: Row): string {
    this.#checkRoot();
    if (row.storage_name !== `${row.id}.bin` || !/^[0-9a-f-]{36}\.bin$/.test(row.storage_name)) throw new RpcError("corrupt", "The shared file record is invalid.");
    const file = this.#safeFile(this.#root, join(this.#root, row.storage_name));
    if (!file) throw new RpcError("unavailable", "This shared file is unavailable."); return file;
  }
  metadata(reference: AttachmentRef, context?: RequestContext): AttachmentInfo {
    this.#context(context); const row = this.#row(AttachmentRef.parse(reference)), info = this.#info(row);
    if (statSync(this.#path(row)).size !== info.size) throw new RpcError("corrupt", "The shared file changed."); return info;
  }
  #refresh(entry: Lease): void {
    clearTimeout(entry.timer ?? undefined); entry.expires = Date.now() + LEASE_MS;
    entry.timer = setTimeout(() => { void this.#closeLease(entry); }, LEASE_MS); entry.timer.unref();
  }
  async open(reference: AttachmentRef, optional?: RequestContext): Promise<AttachmentOpen> {
    const context = this.#context(optional), ref = AttachmentRef.parse(reference), info = this.metadata(ref, context), row = this.#row(ref);
    const owner = context.principal.kind === "device" ? `device:${context.principal.deviceId}` : `local:${context.connectionId}`;
    if (this.#leases.size >= 16 || [...this.#leases.values()].filter((entry) => entry.owner === owner).length >= 2) throw new RpcError("busy", "Two file transfers are already active. Close one and try again.");
    const id = newId(), entry: Lease = { id, context, owner, ref, info, file: open(this.#path(row), constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0)), fd: null, stats: null, reading: false, verified: false, closed: false, closing: null, expires: 0, timer: null, abort: () => { void this.#closeLease(entry); } };
    this.#leases.set(id, entry); context.signal.addEventListener("abort", entry.abort, { once: true });
    try {
      const fd = await entry.file; this.#context(context); if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed."); entry.fd = fd;
      const initial = await fd.stat();
      if (initial.size !== info.size) throw new RpcError("corrupt", "The shared file changed.");
      this.#context(context); if (entry.closed || this.#row(ref).id !== info.id) throw new RpcError("unavailable", "This shared file is no longer available."); entry.stats = initial; this.#refresh(entry);
      return { transferId: id, attachment: info, chunkBytes: ATTACHMENT_CHUNK_BYTES, expiresAt: new Date(entry.expires).toISOString() };
    } catch (error) { await this.#closeLease(entry); if (error instanceof RpcError) throw error; throw new RpcError("unavailable", "This shared file could not be opened."); }
  }
  async #verify(entry: Lease, fd: FileHandle): Promise<void> {
    if (entry.verified) return;
    const hash = createHash("sha256"), buffer = Buffer.alloc(64 * 1024);
    // Verify before releasing bytes, after open has given the client a cancellable handle.
    for (let at = 0; at < entry.info.size;) {
      this.#context(entry.context); if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed.");
      const { bytesRead } = await fd.read(buffer, 0, Math.min(buffer.length, entry.info.size - at), at);
      if (!bytesRead) throw new RpcError("corrupt", "The shared file is incomplete."); hash.update(buffer.subarray(0, bytesRead)); at += bytesRead;
    }
    if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed.");
    if (!entry.stats || !sameFile(entry.stats, await fd.stat()) || hash.digest("hex") !== entry.info.sha256) throw new RpcError("corrupt", "The shared file failed its content check."); entry.verified = true;
  }
  async read(input: { transferId: string; offset: number; length: number }, optional?: RequestContext): Promise<AttachmentChunk> {
    const context = this.#context(optional), entry = this.#leases.get(input.transferId);
    if (!entry || entry.closed || entry.context.connectionId !== context.connectionId || entry.owner !== (context.principal.kind === "device" ? `device:${context.principal.deviceId}` : `local:${context.connectionId}`)) throw new RpcError("forbidden", "This file transfer is not authorized.");
    if (Date.now() >= entry.expires) { await this.#closeLease(entry); throw new RpcError("expired", "This file transfer expired. Open it again."); }
    if (!Number.isSafeInteger(input.offset) || input.offset < 0 || input.offset > entry.info.size || !Number.isSafeInteger(input.length) || input.length < 1 || input.length > ATTACHMENT_CHUNK_BYTES) throw new RpcError("bad_params", "Invalid file byte range.");
    if (entry.reading || !entry.fd || !entry.stats) throw new RpcError("busy", "A file read is already in progress.");
    if (this.#row(entry.ref).id !== entry.info.id) throw new RpcError("unavailable", "This shared file is no longer available.");
    entry.reading = true;
    try {
      const fd = entry.fd;
      if (!sameFile(entry.stats, await fd.stat())) throw new RpcError("corrupt", "The shared file changed.");
      await this.#verify(entry, fd); if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed.");
      const bytes = Buffer.alloc(Math.min(input.length, entry.info.size - input.offset)); let count = 0;
      while (count < bytes.length) { if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed."); const read = await fd.read(bytes, count, bytes.length - count, input.offset + count); if (!read.bytesRead) throw new RpcError("corrupt", "The shared file is incomplete."); count += read.bytesRead; }
      this.#context(context); if (entry.closed) throw new RpcError("forbidden", "This file transfer was closed."); if (!sameFile(entry.stats, await fd.stat()) || this.#row(entry.ref).id !== entry.info.id) throw new RpcError("corrupt", "The shared file changed.");
      this.#refresh(entry); return { transferId: entry.id, revision: entry.info.sha256, offset: input.offset, bytes: bytes.toString("base64"), eof: input.offset + count === entry.info.size };
    } catch (error) { await this.#closeLease(entry); if (error instanceof RpcError) throw error; throw new RpcError("unavailable", "The shared file could not be read."); }
    finally { entry.reading = false; }
  }
  #closeLease(entry: Lease): Promise<void> {
    if (entry.closing) return entry.closing;
    entry.closed = true; clearTimeout(entry.timer ?? undefined); entry.context.signal.removeEventListener("abort", entry.abort);
    entry.closing = entry.file.then((fd) => fd.close()).catch(() => {}).finally(() => { entry.fd = null; this.#leases.delete(entry.id); });
    return entry.closing;
  }
  async closeTransfer(transferId: string, optional?: RequestContext): Promise<void> {
    const context = this.#context(optional), entry = this.#leases.get(transferId); if (!entry) return;
    if (entry.context.connectionId !== context.connectionId || entry.owner !== (context.principal.kind === "device" ? `device:${context.principal.deviceId}` : `local:${context.connectionId}`)) throw new RpcError("forbidden", "This file transfer is not authorized.");
    await this.#closeLease(entry);
  }
  cleanup(): void {
    this.#checkRoot();
    const leased = new Set([...this.#leases.values()].map((entry) => entry.info.id));
    const rows = this.#db.prepare("SELECT a.* FROM attachments a WHERE NOT EXISTS(SELECT 1 FROM message_attachments m WHERE m.attachment_id=a.id)").all() as Row[];
    for (const row of rows) {
      if (leased.has(row.id)) continue;
      if (row.storage_name !== `${row.id}.bin` || !/^[0-9a-f-]{36}\.bin$/.test(row.storage_name)) continue;
      rmSync(join(this.#root, row.storage_name), { force: true }); this.#db.prepare("DELETE FROM attachments WHERE id=?").run(row.id);
    }
    const known = new Set((this.#db.prepare("SELECT storage_name FROM attachments").all() as { storage_name: string }[]).map((row) => row.storage_name));
    for (const name of readdirSync(this.#root)) {
      if (!/^[0-9a-f-]{36}\.bin(?:\.part)?$/.test(name) || known.has(name)) continue;
      const file = join(this.#root, name), stats = lstatSync(file);
      if (stats.isFile() && !stats.isSymbolicLink() && Date.now() - stats.mtimeMs > 24 * 60 * 60 * 1000) rmSync(file, { force: true });
    }
  }
  async close(): Promise<void> { this.#closed = true; await Promise.all([...this.#leases.values()].map((entry) => this.#closeLease(entry))); }
}
