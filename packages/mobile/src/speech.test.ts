import { describe, expect, it, vi } from "vitest";
import { createSpeech, type NativeSpeechPlugin } from "./speech.ts";

function fixture() {
  let event: (value: unknown) => void = () => {};
  const remove = vi.fn(async () => {});
  const plugin: NativeSpeechPlugin = {
    status: vi.fn(async () => ({ engine: "android-on-device", capture: "native", available: true, languages: [{ tag: "en-US", label: "English" }], permission: "prompt", model: { state: "system-managed", bytes: 0, received: 0 } })),
    start: vi.fn(async () => {}), stop: vi.fn(async () => {}), cancel: vi.fn(async () => {}),
    downloadModel: vi.fn(async () => {}), releaseModelRequest: vi.fn(async () => {}),
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
  it("requests the official system-managed download explicitly without an online audio fallback", async () => {
    const { api, plugin } = fixture();
    const dispose = api.onEvent(vi.fn());
    await api.downloadModel();
    expect(plugin.downloadModel).toHaveBeenCalledWith({ requestId: expect.stringMatching(/^[A-Za-z0-9_-]{1,80}$/) });
    await expect(api.cancelModelDownload()).rejects.toThrow();
    await expect(api.removeModel()).rejects.toThrow();
    await expect(api.appendPcm({ sessionId: "one", sequence: 0, pcm: new Uint8Array([0, 0]) })).rejects.toThrow("on-device");
    expect(plugin.start).not.toHaveBeenCalled();
    expect((await api.status()).capture).toBe("native");
    dispose();
  });
  it("releases only the disposed registration's model request, even after a new scope requests preparation", async () => {
    const { api, plugin } = fixture();
    const first = api.onEvent(vi.fn()), other = api.onEvent(vi.fn());
    await api.downloadModel();
    const oldId = vi.mocked(plugin.downloadModel).mock.calls[0][0].requestId;
    first();
    expect(plugin.releaseModelRequest).not.toHaveBeenCalled();
    other();
    const current = api.onEvent(vi.fn());
    await api.downloadModel();
    const currentId = vi.mocked(plugin.downloadModel).mock.calls[1][0].requestId;
    expect(currentId).not.toBe(oldId);
    expect(plugin.releaseModelRequest).toHaveBeenCalledWith({ requestId: oldId });
    expect(plugin.releaseModelRequest).not.toHaveBeenCalledWith({ requestId: currentId });
    current();
    expect(plugin.releaseModelRequest).toHaveBeenLastCalledWith({ requestId: currentId });
  });
  it("cannot request preparation after its subscriber was disposed during listener registration", async () => {
    const { plugin } = fixture();
    let finish: (handle: { remove(): Promise<void> }) => void = () => {};
    vi.mocked(plugin.addListener).mockReturnValue(new Promise((done) => { finish = done; }));
    const api = createSpeech(plugin), dispose = api.onEvent(vi.fn());
    const requesting = api.downloadModel(); void requesting.catch(() => {});
    dispose(); finish({ remove: vi.fn(async () => {}) });
    await expect(requesting).rejects.toThrow("no longer active");
    expect(plugin.downloadModel).not.toHaveBeenCalled();
  });
  it("forwards per-caller release while another subscriber remains and rejects invalid ownership IDs", async () => {
    const { api, plugin } = fixture();
    const settings = api.onEvent(vi.fn()), composer = api.onEvent(vi.fn());
    await api.downloadModel({ requestId: "settings_owner" });
    await api.releaseModelRequest?.({ requestId: "settings_owner" }); settings();
    expect(plugin.releaseModelRequest).toHaveBeenCalledExactlyOnceWith({ requestId: "settings_owner" });
    await api.downloadModel({ requestId: "composer_owner" });
    await expect(api.downloadModel({ requestId: "../invalid" })).rejects.toThrow();
    expect(plugin.downloadModel).toHaveBeenCalledTimes(2);
    composer();
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
