import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
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

const render = (patch: Partial<Parameters<typeof RightPanel>[0]> = {}) =>
  renderToStaticMarkup(<RightPanel bot={bot} messages={[]} pane="details" onPane={() => {}} expanded={false} onExpandedChange={() => {}} room={1400} overlay={false} onHide={() => {}} {...patch} />);

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
