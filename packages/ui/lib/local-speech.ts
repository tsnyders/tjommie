import { Capacitor, registerPlugin } from "@capacitor/core";
import { createSpeech, type NativeSpeechPlugin } from "../../mobile/src/speech.ts";
import { SPEECH_LIMITS, type SpeechApi } from "../../protocol/src/speech.ts";
import { desktop } from "./desktop";

let android: SpeechApi | undefined;
export function localSpeech(): SpeechApi | null {
  if (desktop()?.speech) return desktop()!.speech!;
  if (Capacitor.isNativePlatform() && Capacitor.getPlatform() === "android") return android ??= createSpeech(registerPlugin<NativeSpeechPlugin>("TjommieSpeech"));
  return null;
}

let previous: (() => Promise<void>) | undefined;
export async function claimSpeech(cancel: () => Promise<void>): Promise<() => void> {
  const old = previous;
  previous = cancel;
  await old?.();
  return () => { if (previous === cancel) previous = undefined; };
}

export type PcmCapture = { finish(): Promise<void>; cancel(): Promise<void> };
export async function capturePcm(options: { api: SpeechApi; sessionId: string; deviceId?: string; current: () => boolean; failed: () => void; limit: () => void }): Promise<PcmCapture | null> {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, ...(options.deviceId ? { deviceId: { exact: options.deviceId } } : {}) }, video: false });
  const stopTracks = (): void => { for (const track of stream.getTracks()) track.stop(); };
  if (!options.current()) { stopTracks(); return null; }
  let context: AudioContext | undefined;
  let node: AudioWorkletNode | undefined;
  let source: MediaStreamAudioSourceNode | undefined;
  let cancelled = false;
  let queue = Promise.resolve();
  let outstanding = 0;
  let sequence = 0;
  let received = 0;
  let flushed: (() => void) | undefined;
  const cleanup = async (): Promise<void> => {
    cancelled = true; stopTracks(); source?.disconnect(); node?.disconnect(); node?.port.close();
    if (context && context.state !== "closed") await context.close();
  };
  try {
    context = new AudioContext({ sampleRate: SPEECH_LIMITS.sampleRate });
    if (context.sampleRate !== SPEECH_LIMITS.sampleRate) throw new Error("This microphone cannot provide the supported local audio format.");
    await context.audioWorklet.addModule("/audio/pcm-capture.worklet.js");
    if (!options.current()) { await cleanup(); return null; }
    node = new AudioWorkletNode(context, "tjommie-pcm", { numberOfInputs: 1, numberOfOutputs: 1, channelCount: 1, channelCountMode: "explicit" });
    node.port.onmessage = (event: MessageEvent<unknown>) => {
      if (cancelled || !event.data || typeof event.data !== "object" || !("kind" in event.data)) return;
      if (event.data.kind === "flushed") { flushed?.(); return; }
      if (event.data.kind === "limit") { options.limit(); return; }
      if (event.data.kind !== "pcm" || !("buffer" in event.data) || !(event.data.buffer instanceof ArrayBuffer)) return;
      const pcm = new Uint8Array(event.data.buffer);
      received += pcm.byteLength;
      if (!pcm.byteLength || pcm.byteLength > SPEECH_LIMITS.chunkBytes || pcm.byteLength % 2 || received > SPEECH_LIMITS.audioBytes || outstanding >= 4) { options.failed(); return; }
      const position = sequence++;
      outstanding += 1;
      queue = queue.then(async () => { if (!cancelled) await options.api.appendPcm({ sessionId: options.sessionId, sequence: position, pcm }); }).finally(() => { pcm.fill(0); outstanding -= 1; });
      void queue.catch(() => { if (!cancelled) options.failed(); });
    };
    source = context.createMediaStreamSource(stream);
    source.connect(node); node.connect(context.destination);
    await context.resume();
    if (!options.current()) { await cleanup(); return null; }
    return {
      cancel: cleanup,
      finish: async () => {
        stopTracks();
        try {
          await new Promise<void>((done, failed) => {
            const timer = setTimeout(() => failed(new Error("Could not finish microphone capture.")), 1500);
            flushed = () => { clearTimeout(timer); done(); };
            node!.port.postMessage({ kind: "flush" });
          });
          await queue;
        } finally { await cleanup(); }
      },
    };
  } catch (error) { await cleanup(); throw error; }
}
