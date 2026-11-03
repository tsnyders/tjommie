// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpeechApi, SpeechEvent, SpeechStatus } from "../../../protocol/src/speech";
import { VoiceSection } from "./voice-section";

const bridge = vi.hoisted(() => ({ value: null as SpeechApi | null }));
vi.mock("@/lib/local-speech", () => ({ localSpeech: () => bridge.value }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const roots: Root[] = [];
const initial: SpeechStatus = { engine: "whisper.cpp", capture: "pcm", available: false, permission: "unknown", languages: [{ tag: "en", label: "English" }], model: { state: "missing", bytes: 147964211, received: 0 } };
function speech(initialStatus = initial) {
  let status = initialStatus, listener: ((event: SpeechEvent) => void) | undefined;
  const api: SpeechApi = {
    status: vi.fn(async () => status), downloadModel: vi.fn(async () => {}), cancelModelDownload: vi.fn(async () => {}), removeModel: vi.fn(async () => {}),
    start: vi.fn(async () => {}), appendPcm: async () => {}, stop: async () => {}, cancel: async () => {},
    onEvent: (next) => { listener = next; return () => { listener = undefined; }; },
  };
  bridge.value = api;
  return { api, set: (next: SpeechStatus) => { status = next; listener?.({ kind: "status", status }); }, emit: (event: SpeechEvent) => listener?.(event) };
}
const text = (): string => document.body.textContent ?? "";
const button = (label: string): HTMLButtonElement => {
  const found = [...document.querySelectorAll("button")].find((item) => item.textContent === label);
  if (!found) throw new Error(`Missing button ${label}`);
  return found;
};
const settle = (): Promise<void> => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function mount(): Promise<void> {
  const root = createRoot(document.body.appendChild(document.createElement("div"))); roots.push(root);
  await act(async () => root.render(<VoiceSection />)); await settle();
}
beforeEach(() => { bridge.value = null; localStorage.clear(); });
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  document.body.replaceChildren(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("local voice settings", () => {
  it("degrades honestly without the current-device bridge", async () => {
    await mount();
    expect(text()).toContain("Local dictation is unavailable in this app");
    expect(text()).toContain("there is no cloud fallback");
    expect(button("Refresh voice status").disabled).toBe(true);
  });
  it("reads status without recording and persists supported language choices on this device", async () => {
    const { api, emit } = speech({ ...initial, languages: [{ tag: "en", label: "English" }, { tag: "en-GB", label: "English (UK)" }] });
    await mount();
    expect(api.start).not.toHaveBeenCalled(); expect(api.downloadModel).not.toHaveBeenCalled();
    const picker = document.querySelector<HTMLButtonElement>('[aria-label="Dictation language"]');
    if (!picker) throw new Error("No language picker");
    await act(async () => picker.click());
    const choice = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((item) => item.textContent === "English (UK)")!;
    await act(async () => choice.click());
    expect(localStorage.getItem("tjommie.voice.language")).toBe("en-GB");
    expect(document.querySelector("select")).toBeNull();
    expect(picker.getAttribute("aria-expanded")).toBe("false");
    await act(async () => emit({ kind: "final", sessionId: "elsewhere", text: "SYNTHETIC_TRANSCRIPT" }));
    expect(text()).not.toContain("SYNTHETIC_TRANSCRIPT");
  });
  it("does not enumerate microphone names or ask permission until access was already granted", async () => {
    speech();
    const query = vi.fn(async () => ({ state: "prompt" })), enumerate = vi.fn(), capture = vi.fn();
    vi.stubGlobal("navigator", { permissions: { query }, mediaDevices: { enumerateDevices: enumerate, getUserMedia: capture } });
    await mount();
    await act(async () => button("Refresh microphones").click());
    expect(query).toHaveBeenCalledWith({ name: "microphone" });
    expect(enumerate).not.toHaveBeenCalled(); expect(capture).not.toHaveBeenCalled();
    expect(text()).toContain("Start dictation in a conversation to grant microphone access");
  });
  it("cancels a download and fences its late failure", async () => {
    const fake = speech();
    let rejectDownload: ((error: Error) => void) | undefined;
    vi.mocked(fake.api.downloadModel).mockImplementation(() => new Promise((_accept, reject) => { rejectDownload = reject; }));
    vi.mocked(fake.api.cancelModelDownload).mockImplementation(async () => { fake.set(initial); });
    await mount();
    await act(async () => button("Download English model").click());
    await act(async () => fake.set({ ...initial, model: { ...initial.model, state: "downloading", received: 100 } }));
    await act(async () => button("Cancel model download").click());
    await act(async () => rejectDownload?.(new Error("aborted synthetic")));
    expect(fake.api.cancelModelDownload).toHaveBeenCalledOnce();
    expect(text()).toContain("Model download cancelled."); expect(document.querySelector('[role="alert"]')).toBeNull();
  });
  it("removes only the downloaded device model and offers Android preparation without cancellation/removal", async () => {
    const fake = speech({ ...initial, available: true, model: { ...initial.model, state: "ready" } });
    vi.mocked(fake.api.removeModel).mockImplementation(async () => fake.set(initial));
    await mount();
    await act(async () => button("Remove downloaded model").click());
    expect(fake.api.removeModel).toHaveBeenCalledOnce();
    expect(text()).toContain("Downloaded model removed from this device.");
    await act(async () => fake.set({ ...initial, engine: "android-on-device", capture: "native", model: { state: "system-managed", received: 0, bytes: 0 } }));
    expect(text()).toContain("Tjommie cannot cancel a system download or remove its data");
    expect(button("Request English (United States) download").disabled).toBe(true);
    expect([...document.querySelectorAll("button")].some((item) => /Download English model|Remove downloaded model|Cancel model download/.test(item.textContent ?? ""))).toBe(false);
  });
  it("shows fixed query diagnostics and SDK preparation separately without advertising an installed language or starting audio", async () => {
    const native: SpeechStatus = { engine: "android-on-device", capture: "native", available: false, permission: "granted", languages: [], model: { state: "system-managed", bytes: 0, received: 0 }, problem: { code: "support_uncheckable", message: "This service does not allow support queries (14)." } };
    const fake = speech(native);
    vi.mocked(fake.api.downloadModel).mockImplementation(async () => fake.set({ ...native, available: true, model: { ...native.model, confirmation: { language: "en-US", source: "android-download-success" } } }));
    await mount();
    expect(text()).toContain("does not allow support queries (14)");
    expect(fake.api.downloadModel).not.toHaveBeenCalled();
    await act(async () => button("Request English (United States) download").click());
    expect(fake.api.downloadModel).toHaveBeenCalledOnce(); expect(fake.api.start).not.toHaveBeenCalled();
    expect(text()).toContain("Each Start confirms it again before recording");
    expect(text()).toContain("SDK preparation");
    const languagePicker = document.querySelector<HTMLButtonElement>("button[aria-label='Dictation language']")!;
    expect(languagePicker.disabled).toBe(true);
    expect(languagePicker.textContent).toBe("No supported language available");
    expect(document.querySelector('[role="menuitemradio"]')).toBeNull();
  });
  it("does not label a scheduled system download as completed and releases only its own request when unmounted", async () => {
    const native: SpeechStatus = { engine: "android-on-device", capture: "native", available: false, permission: "granted", languages: [], model: { state: "system-managed", bytes: 0, received: 0 } };
    const fake = speech(native);
    fake.api.releaseModelRequest = vi.fn(async () => {});
    vi.mocked(fake.api.downloadModel).mockImplementation(async () => fake.set({ ...native, problem: { code: "download_scheduled", message: "Android scheduled the download. Completion was not confirmed." } }));
    await mount(); await act(async () => button("Request English (United States) download").click());
    const owner = vi.mocked(fake.api.downloadModel).mock.calls[0][0];
    expect(owner?.requestId).toMatch(/^[A-Za-z0-9_-]{1,80}$/);
    expect(text()).toContain("Completion was not confirmed"); expect(text()).not.toContain("SDK preparation");
    expect(fake.api.start).not.toHaveBeenCalled();
    await act(async () => roots.pop()!.unmount());
    expect(fake.api.releaseModelRequest).toHaveBeenCalledExactlyOnceWith(owner);
  });
});
