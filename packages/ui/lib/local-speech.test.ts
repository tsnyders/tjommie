import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SpeechApi } from "../../protocol/src/speech.ts";
import { capturePcm, claimSpeech } from "./local-speech";

afterEach(() => vi.unstubAllGlobals());

describe("microphone lifecycle", () => {
  it("stops a stream arriving after cancellation before creating an audio graph", async () => {
    let deliver: (stream: MediaStream) => void = () => {};
    const stream = new Promise<MediaStream>((done) => { deliver = done; });
    const getUserMedia = vi.fn(async () => stream);
    const stop = vi.fn();
    const context = vi.fn();
    vi.stubGlobal("navigator", { mediaDevices: { getUserMedia } });
    vi.stubGlobal("AudioContext", context);
    let current = true;
    const capturing = capturePcm({ api: {} as SpeechApi, sessionId: "one", current: () => current, failed: vi.fn(), limit: vi.fn() });
    current = false;
    deliver({ getTracks: () => [{ stop }] } as unknown as MediaStream);
    expect(await capturing).toBeNull();
    expect(stop).toHaveBeenCalledOnce();
    expect(context).not.toHaveBeenCalled();
    expect(getUserMedia).toHaveBeenCalledWith({ audio: { channelCount: 1 }, video: false });
  });
  it("awaits the preceding composer's cancellation and cannot release a newer owner", async () => {
    const first = vi.fn(async () => {});
    const second = vi.fn(async () => {});
    const releaseFirst = await claimSpeech(first);
    const releaseSecond = await claimSpeech(second);
    expect(first).toHaveBeenCalledOnce();
    releaseFirst();
    const releaseThird = await claimSpeech(async () => {});
    expect(second).toHaveBeenCalledOnce();
    releaseSecond(); releaseThird();
  });
});

type PortEvent = { kind: string; buffer?: ArrayBuffer };
type Processor = { port: { onmessage: ((event: { data: { kind: string } }) => void) | null; postMessage: (event: PortEvent) => void }; process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean };
function worklet() {
  const messages: PortEvent[] = [];
  let constructor: (new () => Processor) | undefined;
  runInNewContext(readFileSync(new URL("../public/audio/pcm-capture.worklet.js", import.meta.url), "utf8"), {
    AudioWorkletProcessor: class { port = { onmessage: null, postMessage: (event: PortEvent) => messages.push(event) }; },
    registerProcessor: (_name: string, value: new () => Processor) => { constructor = value; },
  });
  if (!constructor) throw new Error("The worklet was not registered.");
  return { processor: new constructor(), messages };
}

describe("synthetic PCM worklet", () => {
  it("writes clamped little-endian PCM16, emits only silent output, and flushes before acknowledgement", () => {
    const { processor, messages } = worklet();
    const output = new Float32Array(4).fill(1);
    processor.process([[new Float32Array([-2, 2, Number.NaN, 0.5])]], [[output]]);
    processor.port.onmessage!({ data: { kind: "flush" } });
    const view = new DataView(messages[0].buffer!);
    expect([0, 1, 2, 3].map((index) => view.getInt16(index * 2, true))).toEqual([-32768, 32767, 0, 16384]);
    expect([...output]).toEqual([0, 0, 0, 0]);
    expect(messages.at(-1)).toEqual({ kind: "flushed" });
  });
  it("bounds a recording at sixty seconds and rejects further samples", () => {
    const { processor, messages } = worklet();
    processor.process([[new Float32Array(960100)]], []);
    const bytes = messages.reduce((total, message) => total + (message.buffer?.byteLength ?? 0), 0);
    expect(bytes).toBe(1920000);
    expect(messages.filter((message) => message.kind === "limit")).toHaveLength(1);
    const count = messages.length;
    processor.process([[new Float32Array(100)]], []);
    expect(messages).toHaveLength(count);
  });
});
