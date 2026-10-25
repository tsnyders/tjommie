import { describe, expect, it } from "vitest";
import { isPhone, PHONE_BELOW, choosePane, clampWidth, coversChat, isOverlay, panelFor, readWidth, saveWidth, setExpanded, splitterWidth, widthBounds } from "./panel-layout";

describe("panel width", () => {
  it("keeps the chat's minimum beside the panel, and the pane's own minimum", () => {
    expect(widthBounds("browser", 1400)).toEqual({ min: 420, max: 920 });
    expect(widthBounds("details", 1400)).toEqual({ min: 400, max: 920 });
    expect(widthBounds("details", 0).max).toBe(900); // not measured yet
  });

  it("becomes an overlay below 1000 px, with a strip of chat left showing", () => {
    expect([isOverlay(999), isOverlay(1000), isOverlay(0)]).toEqual([true, false, false]);
    expect(widthBounds("browser", 700)).toEqual({ min: 420, max: 652 });
    expect(widthBounds("browser", 450).max).toBe(420); // never below its minimum
  });

  it("clamps a width into its bounds", () => {
    const bounds = { min: 420, max: 920 };
    expect([clampWidth(100, bounds), clampWidth(600.4, bounds), clampWidth(5000, bounds)]).toEqual([420, 600, 920]);
  });

  it("steps with the arrow keys, by 64 with Shift, and jumps with Home and End", () => {
    const bounds = { min: 420, max: 920 };
    expect(splitterWidth("ArrowLeft", false, 560, bounds)).toBe(576);
    expect(splitterWidth("ArrowRight", true, 560, bounds)).toBe(496);
    expect(splitterWidth("ArrowRight", false, 425, bounds)).toBe(420);
    expect(splitterWidth("Home", false, 560, bounds)).toBe(420);
    expect(splitterWidth("End", false, 560, bounds)).toBe(920);
    expect(splitterWidth("a", false, 560, bounds)).toBeNull();
  });

  it("remembers a width per pane, and falls back to the default for nothing or junk", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => void store.set(key, value) };
    expect([readWidth(storage, "browser"), readWidth(storage, "details")]).toEqual([560, 460]);
    saveWidth(storage, "browser", 640.4);
    expect(readWidth(storage, "browser")).toBe(640);
    expect(readWidth(storage, "details")).toBe(460);
    store.set("tjommie.panel-width.details", "junk");
    expect(readWidth(storage, "details")).toBe(460);
  });
});

describe("isPhone", () => {
  it("is a viewport narrower than 640 px, and unknown (0) is not a phone", () => {
    expect(PHONE_BELOW).toBe(640);
    expect([360, 390, 639].map(isPhone)).toEqual([true, true, true]);
    expect([640, 999, 1440, 0].map(isPhone)).toEqual([false, false, false, false]);
  });
});

describe("panel view", () => {
  const docked = { bot: "b1", pane: "browser" as const, expanded: false };

  it("docks the browser when leaving it, so Expand, Details, Browser comes back docked", () => {
    let view = setExpanded(docked, "b1", true);
    view = choosePane(view, "b1", "details");
    expect(view).toEqual({ bot: "b1", pane: "details", expanded: false });
    view = choosePane(view, "b1", "browser");
    expect(view).toEqual(docked);
  });

  it("keeps an expanded browser expanded when the browser is chosen again", () => {
    expect(choosePane(setExpanded(docked, "b1", true), "b1", "browser").expanded).toBe(true);
  });

  it("starts another bot at Details", () => {
    expect(panelFor(docked, "b2")).toEqual({ bot: "b2", pane: "details", expanded: false });
    expect(choosePane(docked, "b2", "browser")).toEqual({ bot: "b2", pane: "browser", expanded: false });
  });

  it("covers the chat for an overlay, or an expanded browser only while the bot has a browser", () => {
    const expanded = { ...docked, expanded: true };
    expect(coversChat(true, false, expanded, true)).toBe(true);
    expect(coversChat(true, false, expanded, false)).toBe(false); // browser turned off while expanded
    expect(coversChat(true, false, docked, true)).toBe(false);
    expect(coversChat(true, true, docked, false)).toBe(true);
    expect(coversChat(false, true, expanded, true)).toBe(false);
  });
});
