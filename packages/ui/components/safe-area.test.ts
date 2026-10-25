import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("../app/globals.css", import.meta.url), "utf8");
const block = css.slice(css.indexOf(".android-app"));

describe("the Android shell's safe areas", () => {
  it("pads each side by the native inset or Capacitor's value, whichever is larger, and by nothing where neither is given", () => {
    for (const side of ["top", "right", "bottom", "left"]) {
      expect(block).toContain(`padding-${side}: max(env(safe-area-inset-${side}, 0px), var(--safe-area-inset-${side}, 0px));`);
    }
  });

  it("applies to the Android app only: no other rule in the sheet pads for the system bars", () => {
    // Four sides, two sources, inside .android-app; and the phone's floating nav, which sits over the shell's padding, clearing the gesture bar.
    expect(css.match(/safe-area-inset/g)?.length).toBe(10);
    expect(css).toContain("bottom: max(20px, calc(max(env(safe-area-inset-bottom, 0px), var(--safe-area-inset-bottom, 0px)) + 10px));");
    expect(css.slice(0, css.indexOf(".android-app")).includes("safe-area-inset")).toBe(false);
  });
});
