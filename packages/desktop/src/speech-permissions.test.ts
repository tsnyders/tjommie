import type { Session, WebContents } from "electron";
import { describe, expect, it, vi } from "vitest";
import { installSpeechPermissions, speechOrigin } from "./speech-permissions.ts";

describe("speech permissions", () => {
  it("allows only a trusted main frame's audio during an active capture lease", () => {
    let capturing = true;
    const window = { getURL: () => "app://tjommie/chat/", isDestroyed: () => false } as unknown as WebContents;
    const check = vi.fn<Session["setPermissionCheckHandler"]>();
    const request = vi.fn<Session["setPermissionRequestHandler"]>();
    installSpeechPermissions({ setPermissionCheckHandler: check, setPermissionRequestHandler: request }, () => window, () => capturing, true);
    const accepts = check.mock.lastCall![0]!;
    const details = { isMainFrame: true, mediaType: "audio" as const, requestingUrl: "app://tjommie/chat/" };
    expect(accepts(window, "media", "app://tjommie", details)).toBe(true);
    for (const change of [{ isMainFrame: false }, { mediaType: "video" as const }, { mediaType: undefined }, { requestingUrl: "https://example.com/" }]) expect(accepts(window, "media", "app://tjommie", { ...details, ...change })).toBe(false);
    expect(accepts(null, "media", "app://tjommie", details)).toBe(false);
    expect(accepts({ ...window } as WebContents, "media", "app://tjommie", details)).toBe(false);
    expect(accepts(window, "display-capture", "app://tjommie", details)).toBe(false);
    const allow = vi.fn();
    const ask = request.mock.lastCall![0]!;
    ask(window, "media", allow, { isMainFrame: true, requestingUrl: "app://tjommie/", mediaTypes: ["audio"] });
    expect(allow).toHaveBeenLastCalledWith(true);
    for (const mediaTypes of [[], ["video"], ["audio", "video"], undefined] as const) {
      ask(window, "media", allow, { isMainFrame: true, requestingUrl: "app://tjommie/", mediaTypes: mediaTypes ? [...mediaTypes] : undefined });
      expect(allow).toHaveBeenLastCalledWith(false);
    }
    capturing = false;
    expect(accepts(window, "media", "app://tjommie", details)).toBe(false);
  });
  it("does not treat lookalike, credential-bearing or wrong-mode origins as trusted", () => {
    expect(speechOrigin("app://tjommie/", true)).toBe(true);
    expect(speechOrigin("http://localhost:3000/chat/", false)).toBe(true);
    for (const value of ["app://tjommie.evil/", "app://user@tjommie/", "app://tjommie:44/", "https://tjommie/", "http://localhost:3000/"]) expect(speechOrigin(value, true)).toBe(false);
  });
});
