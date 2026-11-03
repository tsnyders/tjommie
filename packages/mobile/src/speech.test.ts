import { describe, expect, it, vi } from "vitest";
import { createSpeech, type NativeSpeechPlugin } from "./speech.ts";

function fixture() {
  let event: (value: unknown) => void = () => {};
  const remove = vi.fn(async () => {});
  const plugin: NativeSpeechPlugin = {
    status: vi.fn(async () => ({ engine: "android-on-device", capture: "native", available: true, languages: [{ tag: "en-US", label: "English" }], permission: "prompt", model: { state: "system-managed", bytes: 0, received: 0 } })),
    start: vi.fn(async () => {}), stop: vi.fn(async () => {}), cancel: vi.fn(async () => {}),
    addListener: vi.fn(async (_name, listener) => { event = listener; return { remove }; }),
  };
  return { api: createSpeech(plugin), plugin, event: (value: unknown) => event(value), remove };
}

describe("native local speech bridge", () => {
  it("validates native events, installs the listener before start, and tears it down", async () => {
    const fixtureValue = fixture();
    const received = vi.fn();
    const dispose = fixtureValue.api.onEvent(received);
    await fixtureValue.api.start({ sessionId: "one", language: "en-US" });
    expect(fixtureValue.plugin.addListener).toHaveBeenCalledOnce();
    expect(fixtureValue.plugin.start).toHaveBeenCalledWith({ sessionId: "one", language: "en-US" });
    fixtureValue.event({ kind: "final", sessionId: "one", text: "synthetic text" });
    fixtureValue.event({ kind: "final", sessionId: "one", text: "secret", extra: true });
    fixtureValue.event({ kind: "final", sessionId: "one", text: "" });
    expect(received).toHaveBeenCalledOnce();
    dispose(); await Promise.resolve(); await Promise.resolve();
    expect(fixtureValue.remove).toHaveBeenCalledOnce();
  });
  it("never invokes an online fallback or app-managed system model download", async () => {
    const { api, plugin } = fixture();
    await expect(api.downloadModel()).rejects.toThrow("managed in Android");
    await expect(api.cancelModelDownload()).rejects.toThrow();
    await expect(api.removeModel()).rejects.toThrow();
    await expect(api.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array([0, 0]) })).rejects.toThrow("on-device");
    expect(plugin.start).not.toHaveBeenCalled();
    expect((await api.status()).capture).toBe("native");
  });
  it("cannot start after cancellation while the native listener registration is pending", async () => {
    const { plugin } = fixture();
    let complete: (handle: { remove(): Promise<void> }) => void = () => {};
    vi.mocked(plugin.addListener).mockReturnValue(new Promise((done) => { complete = done; }));
    const api = createSpeech(plugin);
    const dispose = api.onEvent(vi.fn());
    const starting = api.start({ sessionId: "one", language: "en-US" }); void starting.catch(() => {});
    await api.cancel({ sessionId: "one" }); dispose();
    const remove = vi.fn(async () => {}); complete({ remove });
    await expect(starting).rejects.toThrow("cancelled");
    expect(plugin.start).not.toHaveBeenCalled();
    expect(remove).toHaveBeenCalledOnce();
  });
});
