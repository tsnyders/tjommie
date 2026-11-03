// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Dictation } from "@/lib/use-dictation";
import { DictationButton, DictationOptions, DictationStatus } from "./dictation-button";

Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
let root: Root | undefined;
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; document.body.replaceChildren(); });

function voice(change: Partial<Dictation> = {}): Dictation {
  const action = () => vi.fn(async () => {});
  return { status: null, phase: "idle", message: "", issue: "", transcript: "", language: "en", deviceId: "", devices: [], modelWorking: false,
    start: action(), stop: action(), cancel: action(), changed: vi.fn(), setLanguage: vi.fn(), setDevice: vi.fn(), download: action(), cancelDownload: action(), removeModel: action(), insert: vi.fn(), discard: vi.fn(), ...change };
}
describe("dictation controls", () => {
  it("uses explicit non-submit microphone/stop controls and an accessible status", () => {
    expect(renderToStaticMarkup(<DictationButton voice={voice()} />)).toContain('aria-label="Start dictation"');
    expect(renderToStaticMarkup(<DictationButton voice={voice({ phase: "listening" })} />)).toContain('aria-label="Stop dictation"');
    expect(renderToStaticMarkup(<DictationButton voice={voice({ phase: "permission" })} />)).toContain("disabled");
    const html = renderToStaticMarkup(<DictationStatus voice={voice({ phase: "transcribing", message: "Local transcription", transcript: "<synthetic transcript>" })} />);
    expect(html).toContain('role="status"'); expect(html).toContain("Cancel dictation");
    expect(html).toContain("&lt;synthetic transcript&gt;"); expect(html).toContain("Insert transcript");
    expect(html).not.toContain('type="submit"');
  });
  it("leaves no idle footnote or setup panel after a successful dictation", () => {
    const html = renderToStaticMarkup(<DictationStatus voice={voice({ message: "Transcript added to your draft. Review it before sending.", status: { engine: "android-on-device", capture: "native", available: true, languages: [], permission: "granted", model: { state: "system-managed", bytes: 0, received: 0 } } })} />);
    expect(html).toBe("");
  });
  it("lets keyboard users focus a long recovery transcript before explicitly inserting it", async () => {
    const current = voice({ transcript: "Synthetic transcript to review. ".repeat(100) });
    root = createRoot(document.body.appendChild(document.createElement("div")));
    await act(async () => root!.render(<DictationStatus voice={current} />));
    const transcript = document.querySelector<HTMLElement>('[role="region"][aria-label="Transcript to review before insertion"]')!;
    await act(async () => transcript.focus());
    expect(transcript.tabIndex).toBe(0); expect(document.activeElement).toBe(transcript);
    expect(transcript.textContent).toBe(current.transcript);
    expect(current.insert).not.toHaveBeenCalled(); expect(current.start).not.toHaveBeenCalled();
    const insert = [...document.querySelectorAll("button")].find((item) => item.textContent === "Insert transcript")!;
    await act(async () => insert.click());
    expect(current.insert).toHaveBeenCalledOnce();
  });
  it("opens language/model recovery only from an intentional settings control and closes on Escape", async () => {
    const current = voice({ status: { engine: "android-on-device", capture: "native", available: false, languages: [], permission: "granted", model: { state: "system-managed", bytes: 0, received: 0 }, problem: { code: "support_uncheckable", message: "Synthetic SDK diagnostic." } } });
    const submit = vi.fn();
    root = createRoot(document.body.appendChild(document.createElement("div")));
    await act(async () => root!.render(<form onSubmit={(event) => { event.preventDefault(); submit(); }}><DictationOptions voice={current} /></form>));
    expect(document.body.textContent).not.toContain("Synthetic SDK diagnostic");
    const trigger = document.querySelector<HTMLButtonElement>('button[aria-label="Dictation settings"]')!;
    await act(async () => trigger.click());
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(document.body.textContent).toContain("Synthetic SDK diagnostic");
    const download = [...document.querySelectorAll("button")].find((item) => item.textContent === "Request English (United States) download")!;
    await act(async () => download.click());
    expect(current.download).toHaveBeenCalledOnce(); expect(current.start).not.toHaveBeenCalled();
    expect(document.querySelector("button button")).toBeNull();
    await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(submit).not.toHaveBeenCalled();
  });
  it("chooses a language and microphone through the house menus without starting dictation or submitting", async () => {
    const current = voice({ status: { engine: "whisper.cpp", capture: "pcm", available: true, languages: [{ tag: "en", label: "English" }, { tag: "en-GB", label: "English (UK)" }], permission: "granted", model: { state: "ready", bytes: 1, received: 1 } }, devices: [{ deviceId: "desk", groupId: "synthetic", kind: "audioinput", label: "Desk microphone", toJSON: () => ({}) }] });
    const submit = vi.fn();
    root = createRoot(document.body.appendChild(document.createElement("div")));
    await act(async () => root!.render(<form onSubmit={(event) => { event.preventDefault(); submit(); }}><DictationOptions voice={current} /></form>));
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Dictation settings"]')!.click());
    const language = document.querySelector<HTMLButtonElement>('[aria-label="Dictation language"]')!;
    await act(async () => language.click());
    const english = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((item) => item.textContent === "English (UK)")!;
    await act(async () => english.click());
    expect(current.setLanguage).toHaveBeenCalledWith("en-GB");
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Dictation microphone"]')!.click());
    const microphone = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((item) => item.textContent === "Desk microphone")!;
    await act(async () => microphone.click());
    expect(current.setDevice).toHaveBeenCalledWith("desk");
    expect(document.querySelector("button button")).toBeNull();
    expect(current.start).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
  });
});
