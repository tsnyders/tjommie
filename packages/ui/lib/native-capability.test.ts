import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => vi.unstubAllGlobals());

describe("native capability", () => {
  it("has the browser only where the desktop app's native bridge is, whatever the width", async () => {
    vi.resetModules();
    vi.stubGlobal("window", { tjommie: { nativeBrowser: {} }, innerWidth: 360 });
    const { nativeBrowserAvailable, captionClear } = await import("./native-capability");
    expect(nativeBrowserAvailable()).toBe(true);
    expect(captionClear()).toBe("pr-[146px]");
    vi.resetModules();
    vi.stubGlobal("window", { innerWidth: 1600 });
    const web = await import("./native-capability");
    expect(web.nativeBrowserAvailable()).toBe(false);
    expect(web.captionClear()).toBe("pr-4");
    vi.resetModules();
    vi.stubGlobal("window", { tjommie: {} });
    expect((await import("./native-capability")).nativeBrowserAvailable()).toBe(false); // an older bridge without the browser
  });
});
