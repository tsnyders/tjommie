import { execFile, spawn } from "node:child_process";
import { lstat, mkdir, mkdtemp, open, readFile, readdir, rm, type FileHandle } from "node:fs/promises";
import { availableParallelism } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { z } from "zod";
import { SPEECH_LIMITS, SpeechPcm, SpeechSession, SpeechStart, type SpeechApi, type SpeechEvent, type SpeechStatus } from "../../protocol/src/speech.ts";
import { fileDigest, SpeechModel } from "./speech-model.ts";

export type SpeechProcess = (executable: string, args: string[], signal: AbortSignal) => Promise<void>;
export const runSpeechProcess: SpeechProcess = (executable, args, signal) => new Promise((done, failed) => {
  signal.throwIfAborted();
  const child = spawn(executable, args, { windowsHide: true, stdio: "ignore" });
  const kill = (): void => {
    if (!child.pid) return;
    // shortcut: managed shutdown kills the tree; crash-bound termination needs a Windows Job Object.
    if (process.platform === "win32") execFile("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true }, () => {});
    else child.kill("SIGKILL");
  };
  signal.addEventListener("abort", kill, { once: true });
  child.once("error", () => { signal.removeEventListener("abort", kill); failed(new Error("The local speech runtime could not start.")); });
  child.once("exit", (code) => {
    signal.removeEventListener("abort", kill);
    if (signal.aborted) failed(new Error("Local transcription cancelled."));
    else if (code !== 0) failed(new Error("Local transcription failed."));
    else done();
  });
});

const Transcript = z.object({ transcription: z.array(z.object({ text: z.string().max(SPEECH_LIMITS.textCharacters) }).passthrough()).max(4096) }).passthrough();
export function speechText(json: string): string {
  if (Buffer.byteLength(json) > SPEECH_LIMITS.resultBytes) throw new Error("The speech result exceeded its limit.");
  const text = Transcript.parse(JSON.parse(json)).transcription.map((part) => part.text).join("").trim();
  if (!text || text.length > SPEECH_LIMITS.textCharacters || /^\s*[[(](?:blank_audio|silence|music)[\])]\s*$/i.test(text)) throw new Error("No speech was detected. Try again in a quieter place.");
  return text;
}

export function wavHeader(bytes: number): Buffer {
  const header = Buffer.alloc(44);
  header.write("RIFF"); header.writeUInt32LE(36 + bytes, 4); header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16); header.writeUInt16LE(1, 20); header.writeUInt16LE(1, 22);
  header.writeUInt32LE(SPEECH_LIMITS.sampleRate, 24); header.writeUInt32LE(SPEECH_LIMITS.sampleRate * 2, 28);
  header.writeUInt16LE(2, 32); header.writeUInt16LE(16, 34); header.write("data", 36); header.writeUInt32LE(bytes, 40);
  return header;
}

type Active = { id: string; controller: AbortController; phase: "starting" | "listening" | "transcribing"; directory?: string; file?: FileHandle; bytes: number; sequence: number; nonzero: boolean; writes: Promise<void>; work?: Promise<void>; disposing?: Promise<void>; timer?: ReturnType<typeof setTimeout> };
type Options = { runtimePath: string; runtimeSha256?: string | null; dataDir: string; emit: (event: SpeechEvent) => void; platform?: string; run?: SpeechProcess; model?: SpeechModel };

export class SpeechRuntime implements Omit<SpeechApi, "onEvent"> {
  private readonly options: Options;
  readonly model: SpeechModel;
  private active?: Active;
  private initialization?: Promise<void>;
  private runtimeFingerprint?: string;
  private runtimeVerified = false;
  private cancellation?: { id: string; done: Promise<void> };
  private cleanupFailed = false;
  private readonly sessions: string;

  constructor(options: Options) {
    this.options = options;
    this.sessions = join(options.dataDir, "sessions");
    this.model = options.model ?? new SpeechModel({ directory: join(options.dataDir, "models"), changed: () => { void this.status().then((status) => options.emit({ kind: "status", status })).catch(() => {}); } });
  }

  private initialize(): Promise<void> {
    this.initialization ??= (async () => {
      await mkdir(this.sessions, { recursive: true });
      for (const name of await readdir(this.sessions)) {
        if (!/^session-[A-Za-z0-9]+$/.test(name)) continue;
        const path = join(this.sessions, name);
        const stat = await lstat(path);
        if (stat.isDirectory() && !stat.isSymbolicLink()) await this.removeSessionDirectory(path);
      }
    })().catch(() => { this.cleanupFailed = true; throw new Error("Temporary local speech data could not be cleared. Check storage and restart the app."); });
    return this.initialization;
  }

  private async runtimeReady(force = false): Promise<boolean> {
    if ((this.options.platform ?? process.platform) !== "win32" || !this.options.runtimeSha256?.match(/^[a-f0-9]{64}$/)) return false;
    try {
      const stat = await lstat(this.options.runtimePath);
      if (!stat.isFile() || stat.isSymbolicLink() || stat.size <= 0 || stat.size > 128 * 1024 * 1024) return false;
      const fingerprint = `${stat.size}:${stat.mtimeMs}:${stat.ctimeMs}`;
      if (!force && this.runtimeFingerprint === fingerprint) return this.runtimeVerified;
      this.runtimeVerified = await fileDigest(this.options.runtimePath) === this.options.runtimeSha256;
      this.runtimeFingerprint = fingerprint;
      return this.runtimeVerified;
    } catch { return false; }
  }

  async status(): Promise<SpeechStatus> {
    const [runtime, model] = await Promise.all([this.runtimeReady(), this.model.status()]);
    return {
      engine: "whisper.cpp", capture: "pcm", available: runtime && model.state === "ready" && !this.cleanupFailed, permission: "unknown",
      languages: [{ tag: "en", label: "English" }], model,
      ...(this.cleanupFailed ? { problem: { code: "cleanup_failed", message: "Temporary speech data could not be cleared. Check storage and restart the app before dictating again." } }
        : !runtime ? { problem: { code: "runtime_unavailable", message: "Local speech is not included in this app build, or its integrity check failed. Install a build with verified local speech support." } }
        : model.state === "error" ? { problem: { code: "model_error", message: "Local speech model storage or verification failed. Check storage and network, restart the app, then try again." } }
          : model.state !== "ready" ? { problem: { code: "model_missing", message: "Download the verified English speech model to dictate locally." } } : {}),
    };
  }

  isCapturing(): boolean { return this.active?.phase === "listening" && !this.active.controller.signal.aborted; }
  isRecordingAudio(): boolean { return this.isCapturing() && (this.active?.bytes ?? 0) > 0; }
  downloadModel(): Promise<void> { return this.model.downloadModel(); }
  cancelModelDownload(): Promise<void> { return this.model.cancelDownload(); }
  async removeModel(): Promise<void> { await this.cancelAll(); await this.model.remove(); }

  start(input: SpeechStart): Promise<void> {
    const request = SpeechStart.parse(input);
    if (this.active || this.cancellation) throw new Error("Another dictation is still running or stopping.");
    if (this.cleanupFailed) throw new Error("Temporary speech data could not be cleared. Check storage and restart the app.");
    if (request.language !== "en") throw new Error("This local speech model supports English only.");
    const active: Active = { id: request.sessionId, controller: new AbortController(), phase: "starting", bytes: 0, sequence: 0, nonzero: false, writes: Promise.resolve() };
    this.active = active;
    active.work = this.prepare(active);
    return active.work;
  }

  private async prepare(active: Active): Promise<void> {
    try {
      await this.initialize();
      if (!await this.runtimeReady(true)) throw new Error("The verified local speech runtime is unavailable in this build.");
      if (!await this.model.verify()) throw new Error("Download and verify the local speech model first.");
      active.controller.signal.throwIfAborted();
      active.directory = await mkdtemp(join(this.sessions, "session-"));
      active.file = await open(join(active.directory, "input.wav"), "wx", 0o600);
      await active.file.write(wavHeader(0));
      active.controller.signal.throwIfAborted();
      active.phase = "listening";
      active.timer = setTimeout(() => { void this.stop({ sessionId: active.id }).catch(() => {}); }, SPEECH_LIMITS.maxSeconds * 1000);
      this.options.emit({ kind: "session", sessionId: active.id, phase: "listening" });
    } catch {
      if (this.active === active) this.active = undefined;
      await this.dispose(active).catch(() => { this.cleanupFailed = true; });
      throw new Error(active.controller.signal.aborted ? "Dictation cancelled." : "Could not start dictation. Check the verified local runtime, model and temporary storage.");
    }
  }

  async appendPcm(input: SpeechPcm): Promise<void> {
    const chunk = SpeechPcm.parse(input);
    const active = this.active;
    if (!active || active.id !== chunk.sessionId || active.phase !== "listening" || active.controller.signal.aborted || !active.file) throw new Error("This recording is no longer active.");
    if (chunk.sequence !== active.sequence || active.bytes + chunk.pcm.byteLength > SPEECH_LIMITS.audioBytes) {
      await this.cancel({ sessionId: active.id });
      throw new Error("The recording exceeded its limits or arrived out of order.");
    }
    active.sequence += 1;
    active.bytes += chunk.pcm.byteLength;
    const bytes = Buffer.from(chunk.pcm);
    active.nonzero ||= bytes.some((byte) => byte !== 0);
    active.writes = active.writes.then(async () => {
      active.controller.signal.throwIfAborted();
      let offset = 0;
      while (offset < bytes.length) {
        const written = (await active.file!.write(bytes, offset, bytes.length - offset)).bytesWritten;
        if (written <= 0) throw new Error("Temporary audio could not be written.");
        offset += written;
      }
    });
    try { await active.writes; }
    catch { await this.cancel({ sessionId: active.id }).catch(() => {}); throw new Error("Microphone data could not be saved temporarily. Check storage and try again."); }
  }

  async stop(input: { sessionId: string }): Promise<void> {
    const { sessionId } = SpeechSession.parse(input);
    const active = this.active;
    if (!active || active.id !== sessionId || active.phase !== "listening") throw new Error("This recording is no longer active.");
    clearTimeout(active.timer);
    active.phase = "transcribing";
    this.options.emit({ kind: "session", sessionId, phase: "transcribing" });
    active.work = this.transcribe(active);
    await active.work;
  }

  private async transcribe(active: Active): Promise<void> {
    const timeout = setTimeout(() => active.controller.abort(), SPEECH_LIMITS.timeoutMs);
    let text: string | undefined;
    let failed = false;
    try {
      await active.writes;
      active.controller.signal.throwIfAborted();
      if (!active.file || !active.directory || !active.bytes || !active.nonzero) throw new Error("No speech was detected. Try again.");
      await active.file.write(wavHeader(active.bytes), 0, 44, 0);
      await active.file.close(); active.file = undefined;
      const output = join(active.directory, "result");
      await (this.options.run ?? runSpeechProcess)(this.options.runtimePath, ["-m", this.model.path, "-f", join(active.directory, "input.wav"), "-l", "en", "-ng", "-t", String(Math.min(4, availableParallelism())), "-p", "1", "-np", "-oj", "-of", output, "-nth", "0.6", "-lpt", "-1.0", "-sns"], active.controller.signal);
      active.controller.signal.throwIfAborted();
      const path = `${output}.json`;
      const result = await lstat(path);
      if (!result.isFile() || result.isSymbolicLink() || result.size > SPEECH_LIMITS.resultBytes) throw new Error("The speech result exceeded its limit or was not a regular file.");
      text = speechText(await readFile(path, "utf8"));
    } catch {
      failed = true;
    } finally {
      clearTimeout(timeout);
      try { await this.dispose(active); }
      catch { this.cleanupFailed = true; failed = true; text = undefined; }
      if (this.active === active) {
        this.active = undefined;
        if (active.controller.signal.aborted) this.options.emit({ kind: "session", sessionId: active.id, phase: "error", problem: { code: "transcription_timeout", message: "Local transcription took too long. Try a shorter recording." } });
        else if (text && !failed) this.options.emit({ kind: "final", sessionId: active.id, text });
        else this.options.emit({ kind: "session", sessionId: active.id, phase: "error", problem: { code: "transcription_failed", message: "No usable local transcript was produced, or temporary audio cleanup failed. Check storage and try again." } });
      }
    }
  }

  async cancel(input: { sessionId: string }): Promise<void> {
    const { sessionId } = SpeechSession.parse(input);
    const active = this.active;
    if (!active || active.id !== sessionId) { if (this.cancellation?.id === sessionId) await this.cancellation.done; if (this.cleanupFailed) throw new Error("Temporary local speech data could not be cleared. Check storage and restart the app."); return; }
    this.active = undefined;
    clearTimeout(active.timer);
    active.controller.abort();
    const done = (async () => {
      if (active.work) await active.work.catch(() => {});
      try { await this.dispose(active); }
      catch { this.cleanupFailed = true; throw new Error("Temporary local speech data could not be cleared. Check storage and restart the app."); }
      this.options.emit({ kind: "session", sessionId, phase: "cancelled" });
    })();
    this.cancellation = { id: sessionId, done };
    try { await done; } finally { if (this.cancellation?.done === done) this.cancellation = undefined; }
  }

  async cancelAll(): Promise<void> { try { if (this.active) await this.cancel({ sessionId: this.active.id }); if (this.cancellation) await this.cancellation.done; if (this.cleanupFailed) throw new Error("Temporary local speech data could not be cleared. Check storage and restart the app."); } finally { await this.model.cancelDownload(); } }

  private dispose(active: Active): Promise<void> {
    active.disposing ??= (async () => {
      await active.writes.catch(() => {});
      await active.file?.close(); active.file = undefined;
      if (active.directory) await this.removeSessionDirectory(active.directory);
    })();
    return active.disposing;
  }

  private async removeSessionDirectory(path: string): Promise<void> {
    if (dirname(resolve(path)) !== resolve(this.sessions) || !/^session-[A-Za-z0-9]+$/.test(basename(path))) throw new Error("Refusing to remove an unowned speech directory.");
    await rm(path, { recursive: true, force: true });
  }
}
