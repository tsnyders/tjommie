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
