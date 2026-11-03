import { SpeechEvent, SpeechSession, SpeechSessionId, SpeechStart, SpeechStatus, type SpeechApi } from "../../protocol/src/speech.ts";

export type NativeSpeechPlugin = {
  status(): Promise<unknown>;
  start(options: { sessionId: string; language: string }): Promise<void>;
  stop(options: { sessionId: string }): Promise<void>;
  cancel(options: { sessionId: string }): Promise<void>;
  downloadModel(options: { requestId: string }): Promise<void>;
  releaseModelRequest(options: { requestId: string }): Promise<void>;
  addListener(event: "speechEvent", listener: (event: unknown) => void): Promise<{ remove(): Promise<void> }>;
};
type Registration = { active: boolean; ready: Promise<{ remove(): Promise<void> }>; requestId?: string; requesting?: string };

export function createSpeech(plugin: NativeSpeechPlugin): SpeechApi {
  const listeners = new Set<(event: SpeechEvent) => void>();
  let registered: Registration | undefined;
  const pendingStarts = new Set<string>();
  const subscribe = (): Promise<void> => {
    if (!registered) {
      const registration: Registration = { active: true, ready: Promise.resolve({ remove: async () => {} }) };
      registration.ready = plugin.addListener("speechEvent", (input) => {
        const parsed = SpeechEvent.safeParse(input);
        if (registration.active && parsed.success) for (const listener of listeners) listener(parsed.data);
      });
      registered = registration;
    }
    return registered.ready.then(() => {});
  };
  const systemModel = (): Promise<void> => Promise.reject(new Error("Speech language data is managed by Android. Tjommie cannot cancel a system download or remove its language data."));
  return {
    status: async () => SpeechStatus.parse(await plugin.status()),
    downloadModel: async (options) => {
      if (!registered || listeners.size === 0) throw new Error("These voice controls are no longer active.");
      const owner = registered;
      if (owner.requesting) throw new Error("An Android speech download request is already pending.");
      const requestId = SpeechSessionId.parse(options?.requestId ?? crypto.randomUUID());
      owner.requesting = requestId;
      try {
        await owner.ready;
        if (!owner.active || registered !== owner || listeners.size === 0) throw new Error("These voice controls are no longer active.");
        if (owner.requesting !== requestId) throw new Error("These voice controls are no longer active.");
        owner.requestId = requestId;
        await plugin.downloadModel({ requestId });
      } finally { if (owner.requesting === requestId) owner.requesting = undefined; }
    },
    releaseModelRequest: (options) => {
      const requestId = SpeechSessionId.parse(options.requestId);
      if (registered?.requesting === requestId) registered.requesting = undefined;
      return plugin.releaseModelRequest({ requestId });
    },
    cancelModelDownload: systemModel, removeModel: systemModel,
    start: async (options) => {
      const request = SpeechStart.parse(options);
      pendingStarts.add(request.sessionId);
      try {
        await subscribe();
        if (!pendingStarts.has(request.sessionId)) throw new Error("Dictation cancelled.");
        await plugin.start(request);
      } finally { pendingStarts.delete(request.sessionId); }
    },
    stop: (options) => plugin.stop(SpeechSession.parse(options)),
    cancel: (options) => { const request = SpeechSession.parse(options); pendingStarts.delete(request.sessionId); return plugin.cancel(request); },
    appendPcm: async () => { throw new Error("Android captures speech on this device through its native on-device recognizer."); },
    onEvent: (listener) => {
      listeners.add(listener);
      void subscribe().catch(() => {});
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0 && registered) {
          const old = registered; registered = undefined; old.active = false;
          if (old.requestId) void plugin.releaseModelRequest({ requestId: old.requestId }).catch(() => {});
          void old.ready.then((handle) => handle.remove()).catch(() => {});
        }
      };
    },
  };
}
