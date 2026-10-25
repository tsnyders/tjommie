import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const setDialogScrim = vi.fn<(open: boolean) => void>();

beforeEach(() => {
  vi.resetModules(); // each test starts with no dialog open
  setDialogScrim.mockReset();
  vi.stubGlobal("window", { tjommie: { setDialogScrim } });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("which dialogs are full screen on a phone", () => {
  it("makes the wide ones (forms) full screen and leaves the small questions as cards", async () => {
    const { fullOnPhone, FULL_SCREEN_FROM } = await import("./dialog");
    expect(FULL_SCREEN_FROM).toBe(400);
    expect([420, 560, 400].map(fullOnPhone)).toEqual([true, true, true]); // account dialogs, the routine editor
    expect([320, 380].map(fullOnPhone)).toEqual([false, false]); // confirms
  });
});

describe("a dialog's header", () => {
  it("is two rows on a full-screen phone dialog: Close and the actions on top, the title across the full width below", async () => {
    const { dialogHeader } = await import("./dialog");
    const header = dialogHeader(true, true);
    expect(header.bar).toContain("max-sm:flex-wrap");
    expect(header.title).toContain("max-sm:order-last");
    expect(header.title).toContain("max-sm:basis-full");
    expect(header.close).toContain("max-sm:order-first");
    expect(header.actions).toContain("max-sm:ml-auto");
    expect(header.actions).toContain("max-sm:mr-[146px]"); // only the top bar clears the caption buttons, not the title row
    expect(header.bar).not.toContain("146");
    expect(header.title).not.toContain("146");
  });

  it("has no caption room to clear in a web client, and is the same one row as ever on desktop and for small questions", async () => {
    const { dialogHeader } = await import("./dialog");
    expect(dialogHeader(true, false).actions).not.toContain("146");
    for (const small of [dialogHeader(false, true), dialogHeader(false, false)]) {
      for (const part of Object.values(small)) expect(part).not.toContain("max-sm:");
    }
    expect(dialogHeader(true, true).title).toContain("flex-1"); // the desktop row's layout is kept at sm and up
  });
});

describe("the caption buttons under a dialog's scrim", () => {
  it("dim while any dialog is open, and brighten when the last one closes", async () => {
    const { dialogOpened } = await import("./dialog");
    const closeFirst = dialogOpened();
    const closeSecond = dialogOpened();
    expect(setDialogScrim.mock.calls).toEqual([[true]]);
    closeFirst();
    await Promise.resolve();
    expect(setDialogScrim.mock.calls).toEqual([[true]]);
    closeSecond();
    await Promise.resolve();
    expect(setDialogScrim.mock.calls).toEqual([[true], [false]]);
  });

  it("stay dim when a dialog closes and another opens in the same update", async () => {
    const { dialogOpened } = await import("./dialog");
    const closeFirst = dialogOpened();
    closeFirst(); // React runs the closing dialog's cleanup first,
    const closeSecond = dialogOpened(); // then the opening dialog's effect
    await Promise.resolve();
    expect(setDialogScrim.mock.calls).toEqual([[true]]);
    closeSecond();
    await Promise.resolve();
    expect(setDialogScrim.mock.calls).toEqual([[true], [false]]);
  });
});
