import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RightPanel } from "./right-panel";

const bot: Bot = {
  id: "b1",
  name: "Scout",
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "codex",
  model: "gpt-5",
  autonomy: "ask",
  browser: true,
  state: "idle",
} as Bot;

const store = new Map<string, string>();
vi.stubGlobal("localStorage", { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => void store.set(key, value) });
afterEach(() => store.clear());
// The desktop app's bridge, with the native browser: the Browser pane is there at any width.
beforeEach(() => vi.stubGlobal("window", { tjommie: { nativeBrowser: {} } }));

const render = (patch: Partial<Parameters<typeof RightPanel>[0]> = {}) =>
  renderToStaticMarkup(<RightPanel bot={bot} messages={[]} pane="details" onPane={() => {}} expanded={false} onExpandedChange={() => {}} room={1400} overlay={false} onHide={() => {}} {...patch} />);

describe("RightPanel on a phone", () => {
  it("is a full-screen sheet with Close, the Details/Browser switch, and no separator or gear", () => {
    const out = render({ phone: true });
    expect(out).toContain("absolute inset-0");
    expect(out).toContain('aria-label="Close details"');
    expect(out).toContain('aria-label="Bot panel views"');
    expect(out).not.toContain('role="separator"');
    expect(out).not.toContain('aria-label="Bot settings"');
    expect(out).not.toContain('aria-label="Hide panel"');
    expect(out).not.toContain("border-l");
    expect(out).not.toContain("width:460px");
  });

  it("keeps the caption buttons' room in the desktop window, and never offers Expand (the sheet is the whole screen)", () => {
    expect(render({ phone: true })).toContain("pr-[146px]");
    const browser = render({ phone: true, pane: "browser", expanded: true });
    expect(browser).not.toContain("Expand browser");
    expect(browser).not.toContain("Restore browser");
  });

  it("is a sheet, not a dialog, so the native browser inside it is not hidden by it", () => {
    expect(render({ phone: true })).not.toContain('role="dialog"');
  });
});

describe("RightPanel without the desktop app's bridge", () => {
  it("has no Browser tab or pane, and the card says the browser runs on the PC", () => {
    vi.stubGlobal("window", {});
    for (const phone of [true, false]) {
      const out = render({ phone, pane: "browser" });
      expect(out).not.toContain("Bot panel views");
      expect(out).not.toContain('id="pane-browser"');
      expect(out).not.toContain('aria-label="Open browser"');
      expect(out).toContain("Runs on your PC. Open Tjommie on that PC to watch Scout or take over.");
      expect(out).toContain('id="pane-details"');
    }
    expect(render({ phone: true })).toContain("pr-4"); // no caption buttons to clear
  });
});

describe("RightPanel", () => {
  it("always offers Details and Browser, Hide panel and a resize separator", () => {
    const out = render();
    expect(out).toContain('aria-label="Bot panel views"');
    expect(out).toMatch(/role="tab"[^>]*aria-selected="true"[^>]*>Details/);
    expect(out).toMatch(/role="tab"[^>]*aria-selected="false"[^>]*>Browser/);
    expect(out).toContain('aria-label="Hide panel"');
    expect(out).toContain('role="separator"');
    expect(out).toContain('aria-label="Resize panel"');
    expect(out).toMatch(/aria-valuenow="460"[^>]*aria-valuemin="400"[^>]*aria-valuemax="920"/);
  });

  it("doesn't mount the browser until it's opened, then keeps it mounted but hidden while Details shows", () => {
    expect(render()).not.toContain('id="pane-browser"');
    const out = render({ pane: "browser" });
    expect(out).toContain('id="pane-browser"');
    expect(out).toMatch(/id="pane-details"[^>]*hidden/);
    expect(out).toMatch(/aria-valuenow="560"/);
  });

  it("uses the remembered width for the shown pane, within bounds", () => {
    store.set("tjommie.panel-width.browser", "700");
    expect(render({ pane: "browser" })).toMatch(/aria-valuenow="700"/);
    store.set("tjommie.panel-width.browser", "5000");
    expect(render({ pane: "browser" })).toMatch(/aria-valuenow="920"/);
  });

  it("has no switch, and never the browser, for a bot without one", () => {
    const out = render({ bot: { ...bot, browser: false }, pane: "browser" });
    expect(out).not.toContain("Bot panel views");
    expect(out).not.toContain('id="pane-browser"');
  });

  it("covers the content area when the browser is expanded, hiding the separator and keeping the caption buttons clear", () => {
    const out = render({ pane: "browser", expanded: true });
    expect(out).toContain("absolute inset-0");
    expect(out).not.toContain('role="separator"');
    expect(out).toContain("pr-[146px]");
  });

  it("overlays the chat in a narrow window, within the room it has", () => {
    const out = render({ room: 700, overlay: true });
    expect(out).toContain("absolute inset-y-0 right-0");
    expect(out).toMatch(/aria-valuemax="652"/);
  });
});
