import { describe, expect, it, vi } from "vitest";
import { BACK_EVENT, escapeCloses, handleBack, registerSheetBack, sheetOpen } from "./use-sheet";

const escape = { key: "Escape", defaultPrevented: false };

describe("Android Back", () => {
  it("is a named cancelable event that an open sheet takes and closes on", () => {
    expect(BACK_EVENT).toBe("tjommie:back");
    const event = new Event(BACK_EVENT, { cancelable: true });
    let closed = 0;
    handleBack(event, () => (closed += 1));
    expect(closed).toBe(1);
    expect(event.defaultPrevented).toBe(true); // so the app knows a sheet took it and doesn't also go back
  });

  it("leaves it to the sheet on top: one that was already taken is not taken twice", () => {
    const event = new Event(BACK_EVENT, { cancelable: true });
    let closed = 0;
    handleBack(event, () => (closed += 1));
    handleBack(event, () => (closed += 1));
    expect(closed).toBe(1);
  });
});

describe("escapeCloses", () => {
  it("closes a phone's sheet on Escape", () => {
    expect(escapeCloses(escape, true, false, false)).toBe(true);
  });

  it("does nothing on a desktop panel, for other keys, or when the key was already taken", () => {
    expect(escapeCloses(escape, false, false, false)).toBe(false);
    expect(escapeCloses({ key: "Enter", defaultPrevented: false }, true, false, false)).toBe(false);
    expect(escapeCloses({ ...escape, defaultPrevented: true }, true, false, false)).toBe(false);
  });

  it("leaves Escape to what is using it: a typed address, the page, or an open page dialog", () => {
    expect(escapeCloses(escape, true, true, false)).toBe(false);
    expect(escapeCloses(escape, true, false, true)).toBe(false);
  });
});

describe("whether a phone sheet is open (the floating nav hides under one)", () => {
  it("is open from the first sheet's registration until the last one closes", () => {
    vi.stubGlobal("window", new EventTarget());
    try {
      expect(sheetOpen()).toBe(false);
      const members = registerSheetBack(() => {});
      const exchange = registerSheetBack(() => {});
      members();
      expect(sheetOpen()).toBe(true);
      exchange();
      expect(sheetOpen()).toBe(false);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
