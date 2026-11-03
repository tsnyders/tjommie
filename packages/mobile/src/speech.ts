import { SpeechEvent, SpeechSession, SpeechStart, SpeechStatus, type SpeechApi } from "../../protocol/src/speech.ts";

export type NativeSpeechPlugin = {
  status(): Promise<unknown>;
  start(options: { sessionId: string; language: string }): Promise<void>;
  stop(options: { sessionId: string }): Promise<void>;
  cancel(options: { sessionId: string }): Promise<void>;
  addListener(event: "speechEvent", listener: (event: unknown) => void): Promise<{ remove(): Promise<void> }>;
};

export function createSpeech(plugin: NativeSpeechPlugin): SpeechApi {
  const listeners = new Set<(event: SpeechEvent) => void>();
  let registered: { active: boolean; ready: Promise<{ remove(): Promise<void> }> } | undefined;
  const pendingStarts = new Set<string>();
  const subscribe = (): Promise<void> => {
    if (!registered) {
      const registration = { active: true, ready: Promise.resolve({ remove: async () => {} }) };
      registration.ready = plugin.addListener("speechEvent", (input) => {
        const parsed = SpeechEvent.safeParse(input);
        if (registration.active && parsed.success) for (const listener of listeners) listener(parsed.data);
      });
      registered = registration;
    }
    return registered.ready.then(() => {});
  };
  const systemModel = (): Promise<void> => Promise.reject(new Error("Speech language data is managed in Android system settings. Tjommie does not download, cancel or remove system speech models."));
  return {
    status: async () => SpeechStatus.parse(await plugin.status()),
    downloadModel: systemModel, cancelModelDownload: systemModel, removeModel: systemModel,
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
          void old.ready.then((handle) => handle.remove()).catch(() => {});
        }
      };
    },
  };
}
