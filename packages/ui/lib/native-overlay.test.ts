import { describe, expect, it } from "vitest";
import { APP_OVERLAY, coveredByAppOverlay, slotBounds } from "./native-overlay";

describe("coveredByAppOverlay", () => {
  const page = (visibilityState: string, open: string[] = []) => ({ visibilityState, querySelector: (selector: string) => (selector === APP_OVERLAY && open.length > 0 ? {} : null) });

  it("is covered while the window is hidden or an app overlay is open", () => {
    expect(coveredByAppOverlay(page("visible"))).toBe(false);
    expect(coveredByAppOverlay(page("hidden"))).toBe(true);
    expect(coveredByAppOverlay(page("visible", ["menu"]))).toBe(true);
  });

  it("names menus, lists and dialogs, but not the browser's own page dialog", () => {
    for (const role of ["menu", "listbox", '"dialog"', "dialog"]) expect(APP_OVERLAY).toContain(role.replace(/"/g, ""));
    expect(APP_OVERLAY).toContain('[role="alertdialog"]:not([data-page-dialog])');
  });
});

describe("slotBounds", () => {
  it("rounds to whole pixels", () => {
    expect(slotBounds({ x: 10.4, y: 99.6, width: 400.5, height: 300.2 }, false)).toEqual({ x: 10, y: 100, width: 401, height: 300 });
  });

  it("is hidden when covered, empty or unknown", () => {
    expect(slotBounds({ x: 0, y: 0, width: 400, height: 300 }, true)).toBeNull();
    expect(slotBounds({ x: 0, y: 0, width: 0, height: 300 }, false)).toBeNull();
    expect(slotBounds({ x: 0, y: 0, width: 400, height: 0.4 }, false)).toBeNull();
    expect(slotBounds(undefined, false)).toBeNull();
  });

  it("stays inside what the desktop app accepts, never negative", () => {
    expect(slotBounds({ x: -20, y: -5, width: 400, height: 300 }, false)).toEqual({ x: 0, y: 0, width: 400, height: 300 });
    expect(slotBounds({ x: 40000, y: 0, width: 400, height: 300 }, false)).toEqual({ x: 32768, y: 0, width: 0, height: 300 });
  });
});
