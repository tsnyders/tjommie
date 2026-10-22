import { isAbsolute, relative, sep } from "node:path";
import { describe, expect, it } from "vitest";
import { nativeBrowserBounds, nativeBrowserFrameUrl, nativeBrowserProfile, nativeBrowserUrl } from "./native-browser-policy.ts";

describe("native browser policy", () => {
  it("isolates deterministic profiles without allowing bot IDs to choose filesystem paths", () => {
    const root = process.cwd();
    const profile = nativeBrowserProfile(root, "../../outside");
    expect(isAbsolute(profile)).toBe(true);
    expect(relative(root, profile)).toMatch(/^[0-9a-f]{64}$/);
    expect(relative(root, profile).includes(sep)).toBe(false);
    expect(nativeBrowserProfile(root, "../../outside")).toBe(profile);
    expect(nativeBrowserProfile(root, "other")).not.toBe(profile);
  });
  it("allows web addresses and blank pages, refusing privileges and embedded credentials", () => {
    expect(nativeBrowserUrl("https://example.test/path")).toBe("https://example.test/path");
    expect(nativeBrowserUrl("about:blank")).toBe("about:blank");
    for (const url of ["file:///C:/secret", "javascript:alert(1)", "app://tjommie", "chrome://settings", "data:text/html,hi", "https://name:password@example.test"]) expect(() => nativeBrowserUrl(url)).toThrow();
  });
  it("clips browser surfaces to the owner and protects the caption band", () => {
    expect(nativeBrowserBounds({ x: 900, y: 0, width: 400, height: 800 }, 1000, 700)).toEqual({ x: 900, y: 44, width: 100, height: 656 });
    expect(nativeBrowserBounds({ x: 1200, y: 1000, width: 100, height: 100 }, 1000, 700)).toEqual({ x: 1000, y: 700, width: 0, height: 0 });
  });
  it("allows web-created child frames without relaxing main-frame or privileged navigation", () => {
    for (const url of ["about:srcdoc", "blob:https://example.test/fixture", "data:text/html,child"]) {
      expect(nativeBrowserFrameUrl(url, false)).toBe(url);
      expect(() => nativeBrowserFrameUrl(url, true)).toThrow();
    }
    for (const url of ["file:///C:/secret", "chrome://settings", "devtools://devtools", "javascript:alert(1)", "app://tjommie", "about:config"]) {
      expect(() => nativeBrowserFrameUrl(url, false)).toThrow();
    }
  });
});
