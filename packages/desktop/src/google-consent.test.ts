import { describe, expect, it, vi } from "vitest";
import { beginGoogleConsent, googleConsentUrl } from "./google-consent.ts";

function consent() {
  const url = new URL("https://accounts.google.com/o/oauth2/v2/auth");
  url.search = new URLSearchParams({ client_id: "fixture.apps.googleusercontent.com", redirect_uri: "http://127.0.0.1:12345/google/callback", response_type: "code", scope: "openid email https://www.googleapis.com/auth/gmail.readonly", state: "s".repeat(43), code_challenge: "c".repeat(43), code_challenge_method: "S256", access_type: "offline", prompt: "consent" }).toString();
  return url;
}

describe("Google consent's explicit desktop opener", () => {
  it("opens one validated Google URL and returns only the attempt ID", async () => {
    const open = vi.fn(async () => {});
    const result = await beginGoogleConsent({ services: ["gmail"], begin: async () => ({ attempt: "one", url: consent().href }), cancel: vi.fn(), open });
    expect(open).toHaveBeenCalledWith(consent().href);
    expect(result).toEqual({ attempt: "one" });
  });

  it.each(["javascript:alert(1)", "https://accounts.google.com.attacker.invalid/o/oauth2/v2/auth", "http://accounts.google.com/o/oauth2/v2/auth"])("rejects %s", (url) => {
    expect(() => googleConsentUrl(url, ["gmail"])).toThrow();
  });

  it("rejects remote callbacks, writable scopes, duplicate parameters and unselected services", () => {
    for (const [key, value] of [["redirect_uri", "http://attacker.invalid/callback"], ["scope", "https://www.googleapis.com/auth/gmail.modify"], ["code_challenge_method", "plain"], ["scope", "https://www.googleapis.com/auth/drive.readonly"]]) {
      const url = consent(); url.searchParams.set(key, value);
      expect(() => googleConsentUrl(url.href, ["gmail"])).toThrow();
    }
    const repeated = consent(); repeated.searchParams.append("state", "other");
    expect(() => googleConsentUrl(repeated.href, ["gmail"])).toThrow();
  });

  it("cancels the exact attempt if validation or shell opening fails", async () => {
    const cancel = vi.fn(async () => {});
    await expect(beginGoogleConsent({ services: ["gmail"], begin: async () => ({ attempt: "one", url: consent().href }), cancel, open: async () => { throw new Error("OS detail with sensitive input"); } })).rejects.toThrow("could not open");
    expect(cancel).toHaveBeenCalledWith("one");
  });

  it("does not begin authorization for unrecognized services", async () => {
    const begin = vi.fn();
    await expect(beginGoogleConsent({ services: ["mail.write"], begin, cancel: vi.fn(), open: vi.fn() })).rejects.toThrow();
    expect(begin).not.toHaveBeenCalled();
  });
});
