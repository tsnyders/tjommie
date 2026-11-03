// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyDensity, applyTextSize, density, showAppearance, textSize } from "./appearance-settings";
import { themeScript } from "./theme";

afterEach(() => { vi.unstubAllGlobals(); localStorage.clear(); document.documentElement.removeAttribute("data-text-size"); document.documentElement.removeAttribute("data-density"); });

describe("device appearance settings", () => {
  it("uses the system text size and comfortable spacing for absent or invalid stored choices", () => {
    expect(textSize()).toBe("system"); expect(density()).toBe("comfortable");
    localStorage.setItem("tjommie.textSize", "tiny"); localStorage.setItem("tjommie.density", "crowded");
    showAppearance();
    expect(document.documentElement.dataset).toMatchObject({ textSize: "system", density: "comfortable" });
  });
  it("persists locally and applies exactly the same preferences before paint", () => {
    applyTextSize("larger"); applyDensity("compact");
    expect(textSize()).toBe("larger"); expect(density()).toBe("compact");
    document.documentElement.removeAttribute("data-text-size"); document.documentElement.removeAttribute("data-density");
    new Function(themeScript)();
    expect(document.documentElement.dataset).toMatchObject({ textSize: "larger", density: "compact" });
    expect(localStorage.length).toBe(2);
  });
  it("keeps safe defaults when storage is blocked and does not report a failed write as applied", () => {
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } });
    showAppearance();
    expect(textSize()).toBe("system"); expect(density()).toBe("comfortable");
    expect(() => applyTextSize("large")).toThrow("blocked");
    expect(document.documentElement.dataset.textSize).toBe("system");
  });
});
