import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Dictation } from "@/lib/use-dictation";
import { DictationButton, DictationStatus } from "./dictation-button";

function voice(change: Partial<Dictation> = {}): Dictation {
  const action = vi.fn(async () => {});
  return { status: null, phase: "idle", message: "", transcript: "", language: "en", deviceId: "", devices: [], optionsOpen: false,
    start: action, stop: action, cancel: action, changed: vi.fn(), setLanguage: vi.fn(), setDevice: vi.fn(), download: action, cancelDownload: action, removeModel: action, insert: vi.fn(), discard: vi.fn(), ...change };
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
  it("makes Android language management explicit without suggesting app-controlled downloads", () => {
    const html = renderToStaticMarkup(<DictationStatus voice={voice({ optionsOpen: true, status: { engine: "android-on-device", capture: "native", available: false, languages: [], permission: "prompt", model: { state: "system-managed", bytes: 0, received: 0 } } })} />);
    expect(html).toContain("Android speech settings"); expect(html).not.toContain("Download English model");
  });
});
