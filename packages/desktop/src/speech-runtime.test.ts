import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SpeechEvent } from "../../protocol/src/speech.ts";
import { SpeechModel } from "./speech-model.ts";
import { runSpeechProcess, SpeechRuntime, speechText, wavHeader, type SpeechProcess } from "./speech-runtime.ts";

const directories: string[] = [];
afterEach(async () => { for (const path of directories.splice(0)) { expect(dirname(resolve(path))).toBe(resolve(tmpdir())); await rm(path, { recursive: true, force: true }); } });
async function fixture(run?: SpeechProcess) {
  const path = await mkdtemp(join(tmpdir(), "tjommie-speech-runtime-test-")); directories.push(path);
  const content = Buffer.from("synthetic runtime and model");
  const hash = createHash("sha256").update(content).digest("hex");
  const executable = join(path, "synthetic.exe"); await writeFile(executable, content);
  const model = new SpeechModel({ directory: join(path, "models"), source: { bytes: content.length, sha256: hash, url: "https://huggingface.co/synthetic" }, changed: () => {} });
  await mkdir(join(path, "models")); await writeFile(model.path, content);
  const events: SpeechEvent[] = [];
  const execute = vi.fn<SpeechProcess>(run ?? (async (_executable, args) => {
    const input = await readFile(args[args.indexOf("-f") + 1]);
    expect(input.toString("ascii", 0, 4)).toBe("RIFF");
    expect(input.readUInt32LE(40)).toBe(4);
    await writeFile(`${args[args.indexOf("-of") + 1]}.json`, JSON.stringify({ transcription: [{ text: " synthetic words" }] }));
  }));
  const runtime = new SpeechRuntime({ runtimePath: executable, runtimeSha256: hash, dataDir: path, platform: "win32", emit: (event) => events.push(event), model, run: execute });
  return { path, events, execute, runtime, model };
}

describe("supervised local speech", () => {
  it("requires verified resources, writes bounded PCM WAV, invokes local CPU transcription and deletes raw files before final", async () => {
    const { path, runtime, events, execute } = await fixture();
    expect((await runtime.status()).available).toBe(true);
    await runtime.start({ sessionId: "one", language: "en" });
    expect(runtime.isCapturing()).toBe(true);
    await runtime.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array([1, 0, 2, 0]) });
    await runtime.stop({ sessionId: "one" });
    expect(events.at(-1)).toEqual({ kind: "final", sessionId: "one", text: "synthetic words" });
    expect(execute.mock.calls[0][1]).toContain("-ng");
    expect(execute.mock.calls[0][1]).not.toContain("--prompt");
    expect(runtime.isCapturing()).toBe(false);
    expect(await readdir(join(path, "sessions"))).toEqual([]);
  });
  it("keeps unavailable runtime/model states honest and never spawns", async () => {
    const { path, runtime, execute, model } = await fixture();
    await model.remove();
    await expect(runtime.start({ sessionId: "one", language: "en" })).rejects.toThrow("model");
    const missing = new SpeechRuntime({ runtimePath: join(path, "missing.exe"), dataDir: path, emit: () => {}, model });
    expect((await missing.status()).available).toBe(false);
    await expect(missing.start({ sessionId: "two", language: "en" })).rejects.toThrow("runtime");
    expect(execute).not.toHaveBeenCalled();
  });
  it("rechecks the runtime hash before a new recording even after a successful status check", async () => {
    const { runtime, path, execute } = await fixture();
    expect((await runtime.status()).available).toBe(true);
    await writeFile(join(path, "synthetic.exe"), "changed runtime");
    await expect(runtime.start({ sessionId: "one", language: "en" })).rejects.toThrow("runtime");
    expect(execute).not.toHaveBeenCalled();
  });
  it("does not transcribe digital silence and refuses reordered/oversized chunks", async () => {
    const { runtime, execute, events } = await fixture();
    await runtime.start({ sessionId: "one", language: "en" });
    await runtime.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array(10) });
    await runtime.stop({ sessionId: "one" });
    expect(events.some((event) => event.kind === "final")).toBe(false);
    expect(execute).not.toHaveBeenCalled();
    await runtime.start({ sessionId: "two", language: "en" });
    await expect(runtime.appendPcm({ sessionId: "two", sequence: 1, pcm: new Uint8Array([1, 0]) })).rejects.toThrow("out of order");
    expect(runtime.isCapturing()).toBe(false);
    await runtime.start({ sessionId: "three", language: "en" });
    await expect(runtime.appendPcm({ sessionId: "three", sequence: 0, pcm: new Uint8Array(65538) })).rejects.toThrow();
    await runtime.cancelAll();
  });
  it("fences cancelled transcription even if a late synthetic process completes", async () => {
    let complete: () => void = () => {};
    let started: () => void = () => {};
    const pending = new Promise<void>((done) => { complete = done; });
    const beginning = new Promise<void>((done) => { started = done; });
    const { runtime, events, path } = await fixture(async (_path, args) => { started(); await pending; await writeFile(`${args[args.indexOf("-of") + 1]}.json`, JSON.stringify({ transcription: [{ text: "late" }] })); });
    await runtime.start({ sessionId: "one", language: "en" });
    await runtime.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array([1, 0]) });
    const stopping = runtime.stop({ sessionId: "one" }); await beginning;
    const cancelling = runtime.cancel({ sessionId: "one" });
    expect(() => runtime.start({ sessionId: "two", language: "en" })).toThrow("stopping");
    complete(); await cancelling; await stopping;
    expect(events.some((event) => event.kind === "final")).toBe(false);
    expect(await readdir(join(path, "sessions"))).toEqual([]);
  });
  it("suppresses final text and blocks new recordings when temporary audio cleanup fails", async () => {
    const { runtime, events } = await fixture();
    Reflect.set(runtime, "removeSessionDirectory", async () => { throw new Error("synthetic-private-path"); });
    await runtime.start({ sessionId: "one", language: "en" });
    await runtime.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array([1, 0, 2, 0]) });
    await runtime.stop({ sessionId: "one" });
    expect(events.some((event) => event.kind === "final")).toBe(false);
    expect(JSON.stringify(events)).not.toContain("synthetic-private-path");
    expect((await runtime.status()).problem?.code).toBe("cleanup_failed");
    await expect(runtime.cancelAll()).rejects.toThrow("could not be cleared");
    await expect(runtime.cancelAll()).rejects.toThrow("could not be cleared");
    await expect(runtime.cancel({ sessionId: "one" })).rejects.toThrow("could not be cleared");
    expect(() => runtime.start({ sessionId: "two", language: "en" })).toThrow("cleared");
  });
  it("bounds the complete PCM stream, not just each individual chunk", async () => {
    const { runtime, execute } = await fixture();
    await runtime.start({ sessionId: "one", language: "en" });
    for (let sequence = 0; sequence < 29; sequence += 1) await runtime.appendPcm({ sessionId: "one", sequence, pcm: new Uint8Array(65536) });
    await expect(runtime.appendPcm({ sessionId: "one", sequence: 29, pcm: new Uint8Array(65536) })).rejects.toThrow("limits");
    expect(runtime.isCapturing()).toBe(false);
    expect(execute).not.toHaveBeenCalled();
  });
  it("cancels a start waiting on model verification without creating orphan audio", async () => {
    const { runtime, model, path } = await fixture();
    let complete: (ready: boolean) => void = () => {};
    let entered: () => void = () => {};
    const waiting = new Promise<boolean>((done) => { complete = done; });
    const beginning = new Promise<void>((done) => { entered = done; });
    vi.spyOn(model, "verify").mockImplementation(() => { entered(); return waiting; });
    const starting = runtime.start({ sessionId: "one", language: "en" }); void starting.catch(() => {});
    await beginning;
    const cancelling = runtime.cancel({ sessionId: "one" }); complete(true);
    await cancelling; await expect(starting).rejects.toThrow("cancelled");
    expect(await readdir(join(path, "sessions"))).toEqual([]);
  });
  it("terminates its own synthetic child process on cancellation", async () => {
    const controller = new AbortController();
    const running = runSpeechProcess(process.execPath, ["-e", "setInterval(() => {}, 1000)"], controller.signal);
    setTimeout(() => controller.abort(), 100);
    await expect(running).rejects.toThrow("cancelled");
  });
  it("validates output schema/size and no-speech markers", () => {
    expect(speechText('{"transcription":[{"text":" hello"},{"text":" world"}]}')).toBe("hello world");
    for (const json of ["bad", "{}", '{"transcription":[]}', '{"transcription":[{"text":"[BLANK_AUDIO]"}]}', "x".repeat(131073)]) expect(() => speechText(json)).toThrow();
    expect(wavHeader(1920000).readUInt32LE(40)).toBe(1920000);
  });
});
