// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyTheme, followTheme, themeChoice } from "./theme";

afterEach(() => { vi.unstubAllGlobals(); localStorage.clear(); document.documentElement.classList.remove("dark"); });

describe("theme and device appearance startup", () => {
  it("still applies safe appearance defaults when local storage is unavailable", () => {
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } });
    expect(themeChoice()).toBe("dark");
    const stop = followTheme();
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.dataset.textSize).toBe("system");
    stop();
    expect(() => applyTheme("light")).toThrow("blocked");
  });
  it("follows system changes without replacing local text and density choices", () => {
    const media = matchMedia("(prefers-color-scheme: light)");
    vi.stubGlobal("matchMedia", () => media);
    localStorage.setItem("tjommie.theme", "system");
    localStorage.setItem("tjommie.textSize", "large");
    localStorage.setItem("tjommie.density", "compact");
    const stop = followTheme();
    media.dispatchEvent(new Event("change"));
    expect(localStorage.getItem("tjommie.theme")).toBe("system");
    expect(document.documentElement.dataset).toMatchObject({ textSize: "large", density: "compact" });
    stop();
  });
});
