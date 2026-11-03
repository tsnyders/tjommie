import { createHash, randomUUID } from "node:crypto";
import { createReadStream } from "node:fs";
import { lstat, mkdir, open, readdir, rename, rm, statfs } from "node:fs/promises";
import { join } from "node:path";
import type { SpeechStatus } from "../../protocol/src/speech.ts";

export const SPEECH_MODEL = {
  bytes: 147964211,
  sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
  url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin",
} as const;

export async function fileDigest(path: string): Promise<string> {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink()) throw new Error("A speech resource is not a regular file.");
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

type ModelState = SpeechStatus["model"];
type Options = { directory: string; changed: () => void; fetch?: typeof fetch; source?: { bytes: number; sha256: string; url: string } };

export class SpeechModel {
  private readonly options: Options;
  readonly path: string;
  private readonly source;
  private state: ModelState;
  private checked = false;
  private verification?: Promise<boolean>;
  private initialization?: Promise<void>;
  private download?: { controller: AbortController; done: Promise<void> };
  private cleanupFailed = false;

  constructor(options: Options) {
    this.options = options;
    this.source = options.source ?? SPEECH_MODEL;
    this.path = join(options.directory, `${this.source.sha256}.bin`);
    this.state = { state: "missing", bytes: this.source.bytes, received: 0 };
  }

  async status(): Promise<ModelState> {
    if (!this.checked && !this.download) await this.verify().catch(() => {});
    if (this.cleanupFailed) return { state: "error", bytes: this.source.bytes, received: 0 };
    return { ...this.state };
  }

  private initialize(): Promise<void> {
    this.initialization ??= (async () => {
      await mkdir(this.options.directory, { recursive: true });
      const prefix = `.${this.source.sha256}.`;
      for (const name of await readdir(this.options.directory)) {
        if (!name.startsWith(prefix) || !/^[a-f0-9-]{36}\.partial$/.test(name.slice(prefix.length))) continue;
        const path = join(this.options.directory, name);
        const stat = await lstat(path);
        if (stat.isFile() && !stat.isSymbolicLink()) await rm(path);
      }
    })().catch(() => {
      this.cleanupFailed = true; this.checked = true; this.state = { state: "error", bytes: this.source.bytes, received: 0 };
      throw new Error("Temporary speech model data could not be cleared. Check storage and restart the app.");
    });
    return this.initialization;
  }

  verify(): Promise<boolean> {
    if (this.cleanupFailed) return Promise.resolve(false);
    if (this.download) return Promise.resolve(false);
    this.verification ??= this.initialize().then(() => this.check()).finally(() => { this.verification = undefined; });
    return this.verification;
  }

  private async check(): Promise<boolean> {
    let valid = false;
    try { valid = (await lstat(this.path)).size === this.source.bytes && await fileDigest(this.path) === this.source.sha256; }
    catch { /* Missing or corrupt resources remain unavailable. */ }
    this.checked = true;
    this.state = { state: valid ? "ready" : "missing", bytes: this.source.bytes, received: valid ? this.source.bytes : 0 };
    return valid;
  }

  downloadModel(): Promise<void> {
    if (this.cleanupFailed) return Promise.reject(new Error("Temporary speech model data could not be cleared. Check storage and restart the app."));
    if (this.download) return Promise.reject(new Error("A speech model download is already running."));
    const controller = new AbortController();
    const done = this.receive(controller).finally(() => { if (this.download?.controller === controller) this.download = undefined; });
    this.download = { controller, done };
    return done;
  }

  async cancelDownload(): Promise<void> {
    const current = this.download;
    if (current) { current.controller.abort(); await current.done.catch(() => {}); }
    if (this.cleanupFailed) throw new Error("Speech model cleanup failed. Check storage and restart the app.");
  }

  async remove(): Promise<void> {
    await this.cancelDownload();
    if (this.verification) await this.verification;
    await this.initialize();
    try { await rm(this.path, { force: true }); }
    catch { throw new Error("The downloaded speech model could not be removed. Check storage permissions and try again."); }
    this.checked = true;
    this.state = { state: "missing", bytes: this.source.bytes, received: 0 };
    this.options.changed();
  }

  private async receive(controller: AbortController): Promise<void> {
    const partial = join(this.options.directory, `.${this.source.sha256}.${randomUUID()}.partial`);
    let published = false;
    try {
      await this.initialize();
      const storage = await statfs(this.options.directory);
      if (storage.bavail * storage.bsize < this.source.bytes + 1048576) throw new Error("Not enough free storage for the speech model.");
      controller.signal.throwIfAborted();
      this.state = { state: "downloading", bytes: this.source.bytes, received: 0 };
      this.options.changed();
      let url = new URL(this.source.url);
      let response: Response | undefined;
      for (let hop = 0; hop < 5; hop += 1) {
        if (url.protocol !== "https:" || url.username || url.password || (url.port && url.port !== "443") || !/^(?:huggingface\.co|[a-z0-9.-]+\.hf\.co)$/.test(url.hostname)) throw new Error("Untrusted model location.");
        response = await (this.options.fetch ?? fetch)(url, { signal: controller.signal, redirect: "manual" });
        if (![301, 302, 303, 307, 308].includes(response.status)) break;
        const location = response.headers.get("location");
        await response.body?.cancel();
        if (!location) throw new Error("Invalid model redirect.");
        url = new URL(location, url);
        response = undefined;
      }
      if (!response?.ok || !response.body) throw new Error("Model download failed.");
      const length = response.headers.get("content-length");
      if (length !== null && Number(length) !== this.source.bytes) { await response.body.cancel(); throw new Error("Model size did not match."); }
      const output = await open(partial, "wx", 0o600);
      const hash = createHash("sha256");
      try {
        for await (const chunk of response.body) {
          controller.signal.throwIfAborted();
          const percent = Math.floor(this.state.received / this.source.bytes * 100);
          this.state.received += chunk.byteLength;
          if (this.state.received > this.source.bytes) throw new Error("Model size exceeded its limit.");
          hash.update(chunk);
          let offset = 0;
          while (offset < chunk.byteLength) {
            const written = (await output.write(chunk, offset, chunk.byteLength - offset)).bytesWritten;
            if (written <= 0) throw new Error("The model could not be written.");
            offset += written;
          }
          if (Math.floor(this.state.received / this.source.bytes * 100) !== percent) this.options.changed();
        }
        this.state.state = "verifying";
        this.options.changed();
        if (this.state.received !== this.source.bytes || hash.digest("hex") !== this.source.sha256) throw new Error("Model verification failed.");
        await output.sync();
      } finally { await output.close(); }
      controller.signal.throwIfAborted();
      await rename(partial, this.path);
      published = true;
      controller.signal.throwIfAborted();
      this.checked = true;
      this.state = { state: "ready", bytes: this.source.bytes, received: this.source.bytes };
      this.options.changed();
    } catch {
      let cleanupFailed = false;
      if (published && controller.signal.aborted) try { await rm(this.path, { force: true }); } catch { cleanupFailed = true; }
      this.cleanupFailed ||= cleanupFailed;
      this.checked = true;
      this.state = { state: controller.signal.aborted && !cleanupFailed ? "missing" : "error", bytes: this.source.bytes, received: 0 };
      this.options.changed();
      throw new Error(cleanupFailed ? "Cancelled speech model data could not be removed. Check storage and restart the app." : controller.signal.aborted ? "Speech model download cancelled." : "Could not download and verify the speech model. Check storage and network, then try again.");
    } finally {
      try { await rm(partial, { force: true }); }
      catch {
        this.cleanupFailed = true; this.state = { state: "error", bytes: this.source.bytes, received: 0 }; this.options.changed();
        throw new Error("Temporary speech model data could not be cleared. Check storage and restart the app.");
      }
    }
  }
}
