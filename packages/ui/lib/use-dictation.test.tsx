// @vitest-environment happy-dom
import { act, useLayoutEffect, useRef, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpeechApi, SpeechEvent, SpeechStatus } from "../../protocol/src/speech.ts";
import { initialState } from "./state";
import { update } from "./store";
import { useDictation, type Dictation } from "./use-dictation";

const fixture = vi.hoisted(() => ({ api: null as SpeechApi | null, capture: vi.fn() }));
vi.mock("./local-speech", () => ({ localSpeech: () => fixture.api, capturePcm: fixture.capture, claimSpeech: async () => () => {} }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const ready: SpeechStatus = { engine: "android-on-device", capture: "native", available: true, languages: [{ tag: "en-US", label: "English" }], permission: "prompt", model: { state: "system-managed", bytes: 0, received: 0 } };
const permissionRequired: SpeechStatus = { ...ready, available: false, languages: [], problem: { code: "permission_required", message: "Allow microphone access when you start dictation." } };
let emit: (event: SpeechEvent) => void = () => {};
let voice: Dictation;
let root: Root | undefined;

function Harness({ target = "one" }: { target?: string }) {
  const [text, setText] = useState("before OLD after");
  const field = useRef<HTMLInputElement>(null);
  const currentVoice = useDictation("chat", target, field, text, setText);
  useLayoutEffect(() => { voice = currentVoice; });
  return <div><input ref={field} value={text} readOnly /><button onClick={() => void currentVoice.start()}>Start</button><button onClick={() => void currentVoice.stop()}>Stop</button><button onClick={() => void currentVoice.cancel()}>Cancel</button><button onClick={() => { currentVoice.changed("typed draft"); setText("typed draft"); }}>Type</button><button onClick={() => currentVoice.insert()}>Insert</button><p>{currentVoice.transcript}</p></div>;
}
async function mount() { root = createRoot(document.body.appendChild(document.createElement("div"))); await act(async () => root!.render(<Harness />)); document.querySelector("input")!.setSelectionRange(7, 10); }
async function click(label: string) { await act(async () => [...document.querySelectorAll("button")].find((button) => button.textContent === label)!.click()); }
async function event(value: SpeechEvent) { await act(async () => emit(value)); }
beforeEach(() => {
  update(() => initialState);
  localStorage.clear();
  fixture.capture.mockReset();
  fixture.api = {
    status: vi.fn(async () => ready), start: vi.fn(async () => {}), stop: vi.fn(async () => {}), cancel: vi.fn(async () => {}),
    downloadModel: vi.fn(async () => {}), cancelModelDownload: vi.fn(async () => {}), removeModel: vi.fn(async () => {}), appendPcm: vi.fn(async () => {}),
    onEvent: (listener) => { emit = listener; return () => {}; },
  };
});
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; document.body.replaceChildren(); });

describe("dictation composer hook", () => {
  it("uses SDK preparation separately from the installed list on each explicit Start without automatic capture", async () => {
    const prepared: SpeechStatus = { ...ready, permission: "granted", languages: [], model: { ...ready.model, confirmation: { language: "en-US", source: "android-download-success" } }, problem: { code: "support_uncheckable", message: "The service does not allow support queries (14)." } };
    vi.mocked(fixture.api!.status).mockResolvedValue(prepared);
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockImplementation(() => new Promise((done) => { finish = done; }));
    await mount();
    expect(fixture.api!.start).not.toHaveBeenCalled();
    await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledWith({ sessionId: expect.any(String), language: "en-US" });
    expect(voice.phase).toBe("permission"); expect(voice.status?.languages).toEqual([]);
    await act(async () => finish()); await click("Cancel"); await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledTimes(2);
    expect(fixture.capture).not.toHaveBeenCalled();
    await click("Cancel"); await act(async () => finish());
    expect(voice.phase).toBe("idle");
  });
  it("does not carry a model action's delayed status into another host or conversation", async () => {
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.downloadModel).mockReturnValue(new Promise((done) => { finish = done; }));
    await mount();
    let requested: Promise<void> = Promise.resolve();
    await act(async () => { requested = voice.download(); });
    await act(async () => root!.render(<Harness target="two" />));
    vi.mocked(fixture.api!.status).mockResolvedValue({ ...ready, available: false, languages: [], problem: { code: "download_scheduled", message: "Old scope scheduled." } });
    await act(async () => { finish(); await requested; });
    expect(voice.message).not.toContain("Old scope");
    expect(voice.status?.available).toBe(true);
    expect(fixture.api!.start).not.toHaveBeenCalled();
  });
  it("releases the old conversation's preparation ID without cancelling a newer conversation's request", async () => {
    fixture.api!.releaseModelRequest = vi.fn(async () => {});
    await mount(); await act(async () => voice.download());
    const first = vi.mocked(fixture.api!.downloadModel).mock.calls[0][0];
    await act(async () => root!.render(<Harness target="two" />));
    expect(fixture.api!.releaseModelRequest).toHaveBeenCalledExactlyOnceWith(first);
    await act(async () => voice.download());
    const second = vi.mocked(fixture.api!.downloadModel).mock.calls[1][0];
    expect(first?.requestId).not.toBe(second?.requestId);
    expect(fixture.api!.releaseModelRequest).not.toHaveBeenCalledWith(second);
    expect(fixture.api!.start).not.toHaveBeenCalled();
  });
  it("requests Android permission with the saved English intent without advertising it as installed", async () => {
    localStorage.setItem("tjommie.voice.language", "en-GB");
    vi.mocked(fixture.api!.status).mockResolvedValue(permissionRequired);
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockReturnValue(new Promise<void>((done) => { finish = done; }));
    await mount(); await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledWith({ sessionId: expect.any(String), language: "en-GB" });
    expect(voice.phase).toBe("permission"); expect(voice.status?.languages).toEqual([]); expect(voice.language).toBe("");
    expect(fixture.capture).not.toHaveBeenCalled();
    const confirmed: SpeechStatus = { ...ready, permission: "granted", languages: [{ tag: "en-GB", label: "English (United Kingdom)" }] };
    vi.mocked(fixture.api!.status).mockResolvedValue(confirmed);
    await act(async () => finish());
    expect(voice.phase).toBe("listening"); expect(voice.status).toEqual(confirmed); expect(voice.language).toBe("en-GB");
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it.each(["", "af-ZA", "en-" + "a".repeat(50)])("uses en-US only as a desired first-permission intent when preference %s is invalid", async (saved) => {
    localStorage.setItem("tjommie.voice.language", saved);
    vi.mocked(fixture.api!.status).mockResolvedValue(permissionRequired);
    await mount(); await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledWith({ sessionId: expect.any(String), language: "en-US" });
    expect(voice.status?.languages).toEqual([]); expect(voice.language).toBe("");
    expect(fixture.capture).not.toHaveBeenCalled();
  });
  it.each([
    { ...permissionRequired, problem: { code: "permission_denied", message: "Microphone access was denied." }, permission: "denied" as const },
    { ...permissionRequired, permission: "denied" as const },
    { ...permissionRequired, permission: "granted" as const },
    { ...permissionRequired, engine: "whisper.cpp" as const, capture: "pcm" as const },
    { ...permissionRequired, problem: { code: "on_device_unavailable", message: "No explicit on-device engine." } },
    { ...permissionRequired, permission: "granted" as const, problem: { code: "language_unverified", message: "No installed English language." } },
  ])("does not bypass another unavailable gate: $engine/$permission/$problem.code", async (status) => {
    vi.mocked(fixture.api!.status).mockResolvedValue(status);
    await mount(); await click("Start");
    expect(fixture.api!.start).not.toHaveBeenCalled(); expect(fixture.capture).not.toHaveBeenCalled(); expect(voice.phase).toBe("idle");
  });
  it("refreshes confirmed languages after the first permission attempt fails and uses the confirmed language on retry", async () => {
    vi.mocked(fixture.api!.status).mockResolvedValueOnce(permissionRequired).mockResolvedValueOnce(permissionRequired).mockResolvedValue({ ...ready, permission: "granted", languages: [{ tag: "en-GB", label: "English (United Kingdom)" }] });
    vi.mocked(fixture.api!.start).mockRejectedValueOnce(new Error("The desired en-US language is not installed."));
    await mount(); await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledOnce(); expect(voice.phase).toBe("idle"); expect(voice.language).toBe("en-GB");
    expect(voice.status?.permission).toBe("granted");
    await click("Start");
    expect(vi.mocked(fixture.api!.start).mock.calls[1][0].language).toBe("en-GB"); expect(voice.phase).toBe("listening");
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it("cannot capture or insert when the first permission result arrives after cancellation", async () => {
    vi.mocked(fixture.api!.status).mockResolvedValue(permissionRequired);
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockReturnValue(new Promise<void>((done) => { finish = done; }));
    await mount(); await click("Start");
    expect(fixture.api!.start).toHaveBeenCalledOnce(); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await click("Cancel"); await act(async () => finish()); await event({ kind: "final", sessionId: id, text: "late permission" });
    expect(voice.phase).toBe("idle"); expect(fixture.capture).not.toHaveBeenCalled();
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it("inserts final local text at the captured range without sending", async () => {
    await mount(); await click("Start");
    const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    expect(voice.phase).toBe("listening");
    await click("Stop"); await event({ kind: "final", sessionId: id, text: "spoken words" });
    expect(document.querySelector("input")!.value).toBe("before spoken words after");
    expect(voice.issue).toBe("");
    await event({ kind: "final", sessionId: id, text: "duplicate" });
    expect(document.querySelector("input")!.value).not.toContain("duplicate");
  });
  it("keeps an explicit failure separate from idle success instructions and clears it before retrying", async () => {
    vi.mocked(fixture.api!.start).mockRejectedValueOnce(new Error("Synthetic start failure"));
    await mount(); await click("Start");
    expect(voice.issue).toContain("Couldn’t start local dictation");
    await click("Start");
    expect(voice.issue).toBe("");
    expect(voice.phase).toBe("listening");
  });
  it("does not show a previous conversation's failed-start message after its cleanup resolves", async () => {
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockRejectedValueOnce(new Error("Synthetic start failure"));
    vi.mocked(fixture.api!.cancel).mockReturnValueOnce(new Promise((done) => { finish = done; }));
    await mount(); await click("Start");
    await act(async () => root!.render(<Harness target="two" />));
    await act(async () => finish());
    expect(voice.issue).toBe("");
    expect(voice.phase).toBe("idle");
  });
  it("keeps typed edits and requires an explicit insertion when the original range changed", async () => {
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await click("Type"); await click("Stop"); await event({ kind: "final", sessionId: id, text: " spoken" });
    expect(document.querySelector("input")!.value).toBe("typed draft");
    expect(voice.transcript).toBe(" spoken");
    document.querySelector("input")!.setSelectionRange(11, 11);
    await click("Insert"); expect(document.querySelector("input")!.value).toBe("typed draft spoken");
  });
  it("does not overwrite an identical selected replacement that produces no change event", async () => {
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await act(async () => document.querySelector("input")!.dispatchEvent(new InputEvent("beforeinput", { inputType: "insertText", data: "OLD", bubbles: true })));
    await click("Stop"); await event({ kind: "final", sessionId: id, text: "spoken" });
    expect(document.querySelector("input")!.value).toBe("before OLD after");
    expect(voice.transcript).toBe("spoken");
  });
  it("does not display or insert a conflict transcript after changing host", async () => {
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await click("Type"); await click("Stop"); await event({ kind: "final", sessionId: id, text: "old host transcript" });
    const staleInsert = voice.insert;
    await act(async () => update((state) => ({ ...state, host: "another-host" })));
    expect(voice.transcript).toBe("");
    await act(async () => staleInsert());
    expect(document.querySelector("input")!.value).toBe("typed draft");
  });
  it("fences a start resolving after cancellation and ignores its late result", async () => {
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockReturnValue(new Promise<void>((done) => { finish = done; }));
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await click("Cancel"); await act(async () => finish());
    await event({ kind: "final", sessionId: id, text: "late" });
    expect(document.querySelector("input")!.value).toBe("before OLD after");
    expect(fixture.capture).not.toHaveBeenCalled();
  });
  it("does not cancel the initial permission request merely because its system dialog blurs the window", async () => {
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.start).mockReturnValue(new Promise<void>((done) => { finish = done; }));
    await mount(); await click("Start");
    await act(async () => window.dispatchEvent(new Event("blur")));
    expect(fixture.api!.cancel).not.toHaveBeenCalled();
    await act(async () => finish()); expect(voice.phase).toBe("listening");
    await click("Cancel");
  });
  it("cancels on host switch and rejects results for the previous conversation", async () => {
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await act(async () => update((state) => ({ ...state, host: "another-host" })));
    await event({ kind: "final", sessionId: id, text: "late" });
    expect(fixture.api!.cancel).toHaveBeenCalledWith({ sessionId: id });
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it("cancels on a conversation change even if React reuses the composer instance", async () => {
    await mount(); await click("Start"); const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await act(async () => root!.render(<Harness target="two" />));
    await event({ kind: "final", sessionId: id, text: "previous conversation" });
    expect(fixture.api!.cancel).toHaveBeenCalledWith({ sessionId: id });
    expect(voice.phase).toBe("idle");
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it("does not start on a new conversation after waiting for the preceding cancellation", async () => {
    await mount(); await click("Start");
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.cancel).mockReturnValue(new Promise<void>((done) => { finish = done; }));
    await click("Start");
    await act(async () => root!.render(<Harness target="two" />));
    await act(async () => finish());
    expect(fixture.api!.start).toHaveBeenCalledOnce();
    expect(voice.phase).toBe("idle");
  });
  it("preserves edits made while a new recording waits for the previous cancellation", async () => {
    await mount(); await click("Start");
    let finish: () => void = () => {};
    vi.mocked(fixture.api!.cancel).mockReturnValueOnce(new Promise<void>((done) => { finish = done; }));
    await click("Start"); await click("Type"); await act(async () => finish());
    const id = vi.mocked(fixture.api!.start).mock.calls[1][0].sessionId;
    await click("Stop"); await event({ kind: "final", sessionId: id, text: "next words" });
    expect(document.querySelector("input")!.value).toBe("typed draft");
    expect(voice.transcript).toBe("next words");
  });
  it("stops a capture handle that arrives after cancellation", async () => {
    vi.mocked(fixture.api!.status).mockResolvedValue({ ...ready, capture: "pcm", engine: "whisper.cpp" });
    let finish: (capture: { cancel(): Promise<void>; finish(): Promise<void> }) => void = () => {};
    fixture.capture.mockReturnValue(new Promise((done) => { finish = done; }));
    await mount(); await click("Start"); await click("Cancel");
    const cancel = vi.fn(async () => {});
    await act(async () => finish({ cancel, finish: vi.fn(async () => {}) }));
    expect(cancel).toHaveBeenCalledOnce();
    expect(voice.phase).toBe("idle");
  });
  it("cancels listening on window backgrounding without changing the draft", async () => {
    await mount(); await click("Start");
    const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await act(async () => window.dispatchEvent(new Event("blur")));
    expect(fixture.api!.cancel).toHaveBeenCalledWith({ sessionId: id });
    expect(voice.phase).toBe("idle");
    expect(document.querySelector("input")!.value).toBe("before OLD after");
  });
  it("dictates into a cached draft while the host PC is offline", async () => {
    await act(async () => update((state) => ({ ...state, connection: { status: "offline", reason: "host_offline" } })));
    await mount(); await click("Start");
    const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    await click("Stop"); await event({ kind: "final", sessionId: id, text: "offline words" });
    expect(document.querySelector("input")!.value).toBe("before offline words after");
  });
});
