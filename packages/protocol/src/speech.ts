import { z } from "zod";

// Current-device bridge only: speech never travels over the daemon/remote wire.
export const SPEECH_LIMITS = { sampleRate: 16000, maxSeconds: 60, chunkBytes: 65536, audioBytes: 1920000, resultBytes: 131072, textCharacters: 8192, timeoutMs: 120000 } as const;
export const SpeechSessionId = z.string().regex(/^[A-Za-z0-9_-]{1,80}$/);
export const SpeechProblem = z.object({ code: z.string().min(1).max(60), message: z.string().min(1).max(300) }).strict();
export type SpeechProblem = z.infer<typeof SpeechProblem>;
export const SpeechStatus = z.object({
  engine: z.enum(["whisper.cpp", "android-on-device"]),
  capture: z.enum(["pcm", "native"]),
  available: z.boolean(),
  languages: z.array(z.object({ tag: z.string().min(1).max(40), label: z.string().min(1).max(80) }).strict()).max(100),
  permission: z.enum(["unknown", "prompt", "granted", "denied"]),
  model: z.object({
    state: z.enum(["missing", "downloading", "verifying", "ready", "error", "system-managed"]),
    bytes: z.number().int().nonnegative(), received: z.number().int().nonnegative(),
    // Preparation history for this foreground page; each Start still needs a fresh SDK confirmation.
    confirmation: z.object({ language: z.literal("en-US"), source: z.literal("android-download-success") }).strict().optional(),
  }).strict(),
  problem: SpeechProblem.optional(),
}).strict();
export type SpeechStatus = z.infer<typeof SpeechStatus>;
export const SpeechStart = z.object({ sessionId: SpeechSessionId, language: z.string().min(1).max(40) }).strict();
export type SpeechStart = z.infer<typeof SpeechStart>;
export const SpeechSession = z.object({ sessionId: SpeechSessionId }).strict();
export const SpeechPcm = z.object({
  sessionId: SpeechSessionId, sequence: z.number().int().nonnegative(),
  pcm: z.instanceof(Uint8Array).refine((bytes) => bytes.byteLength > 0 && bytes.byteLength <= SPEECH_LIMITS.chunkBytes && bytes.byteLength % 2 === 0, "Invalid PCM chunk."),
}).strict();
export type SpeechPcm = z.infer<typeof SpeechPcm>;
export const SpeechEvent = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("status"), status: SpeechStatus }).strict(),
  z.object({ kind: z.literal("session"), sessionId: SpeechSessionId, phase: z.enum(["listening", "transcribing", "cancelled", "error"]), problem: SpeechProblem.optional() }).strict(),
  z.object({ kind: z.literal("final"), sessionId: SpeechSessionId, text: z.string().min(1).max(SPEECH_LIMITS.textCharacters) }).strict(),
]);
export type SpeechEvent = z.infer<typeof SpeechEvent>;
export type SpeechApi = {
  status(): Promise<SpeechStatus>;
  downloadModel(options?: { requestId: string }): Promise<void>;
  // Optional current-device observer cleanup. It never cancels Android's system download.
  releaseModelRequest?(options: { requestId: string }): Promise<void>;
  cancelModelDownload(): Promise<void>;
  removeModel(): Promise<void>;
  start(options: SpeechStart): Promise<void>;
  appendPcm(options: SpeechPcm): Promise<void>;
  stop(options: { sessionId: string }): Promise<void>;
  cancel(options: { sessionId: string }): Promise<void>;
  onEvent(listener: (event: SpeechEvent) => void): () => void;
};
