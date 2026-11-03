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
  fixture.capture.mockReset();
  fixture.api = {
    status: vi.fn(async () => ready), start: vi.fn(async () => {}), stop: vi.fn(async () => {}), cancel: vi.fn(async () => {}),
    downloadModel: vi.fn(async () => {}), cancelModelDownload: vi.fn(async () => {}), removeModel: vi.fn(async () => {}), appendPcm: vi.fn(async () => {}),
    onEvent: (listener) => { emit = listener; return () => {}; },
  };
});
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; document.body.replaceChildren(); });

describe("dictation composer hook", () => {
  it("inserts final local text at the captured range without sending", async () => {
    await mount(); await click("Start");
    const id = vi.mocked(fixture.api!.start).mock.calls[0][0].sessionId;
    expect(voice.phase).toBe("listening");
    await click("Stop"); await event({ kind: "final", sessionId: id, text: "spoken words" });
    expect(document.querySelector("input")!.value).toBe("before spoken words after");
    await event({ kind: "final", sessionId: id, text: "duplicate" });
    expect(document.querySelector("input")!.value).not.toContain("duplicate");
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
