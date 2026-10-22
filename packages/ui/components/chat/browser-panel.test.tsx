import type { BrowserState } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { BrowserView } from "@/lib/browser-session";
import type { Bot } from "@tjommie/protocol";
import { BrowserChrome, BrowserPanel, type BrowserControls } from "./browser-panel";

const controls: BrowserControls = { input: vi.fn(), releaseAll: vi.fn(), setControl: vi.fn(), navigate: vi.fn(), history: vi.fn(), resize: vi.fn(), dialog: vi.fn(), tab: vi.fn() };
const state = (patch: Partial<BrowserState> = {}): BrowserState => ({
  botId: "b1",
  generation: "g1",
  control: "bot",
  activePageId: "p1",
  tabs: [{ id: "p1", url: "https://example.com/", title: "Example" }],
  dialog: null,
  ...patch,
});
const html = (view: BrowserView, expanded = false) => renderToStaticMarkup(<BrowserChrome view={view} botName="Scout" controls={controls} expanded={expanded} onToggleExpanded={() => {}} onRetry={() => {}} />);
const shown = (patch: Partial<BrowserState> = {}): BrowserView => ({ state: state(patch), frame: { data: "AAAA", mime: "image/jpeg", width: 800, height: 600, sequence: 1 }, error: null });

describe("BrowserChrome", () => {
  it("labels its parts and offers Take control while the bot drives", () => {
    const out = html(shown());
    for (const label of ["Expand browser", "Browser address", "Browser viewport", "Browser tab Example", "Back", "Forward", "Reload"]) expect(out).toContain(`aria-label="${label}"`);
    expect(out).toContain("Go");
    expect(out).toContain("Take control");
    expect(out).not.toContain("Resume bot");
    expect(out).toContain("Scout is using this browser");
    expect(out).toContain("data:image/jpeg;base64,AAAA");
  });

  it("offers Restore browser while expanded", () => {
    expect(html(shown(), true)).toContain('aria-label="Restore browser"');
  });

  it("states what the viewport's picture covers, for diagnostics", () => {
    const out = html({ ...shown(), frame: { data: "AAAA", mime: "image/jpeg", width: 1280, height: 720, sequence: 7, viewportWidth: 1920, viewportHeight: 1080 } as BrowserView["frame"] });
    expect(out).toContain('data-viewport-width="1920"');
    expect(out).toContain('data-viewport-height="1080"');
    expect(out).toContain('data-frame-sequence="7"');
    expect(out).toContain("object-contain");
  });

  it("says Google may block sign-in only on Google's rejected page", () => {
    const rejected = shown({ tabs: [{ id: "p1", url: "https://accounts.google.com/v3/signin/rejected?x=1", title: "Sign in" }] });
    expect(html(rejected)).toContain("Google may block sign-in in automated browsers.");
    expect(html(shown())).not.toContain("Google may block");
  });

  it("offers Resume bot while the user drives, and enables the controls", () => {
    const out = html(shown({ control: "human" }));
    expect(out).toContain("Resume bot");
    expect(out).not.toContain("Take control");
    expect(out).not.toMatch(/aria-label="Reload"[^>]*disabled/);
    expect(out).not.toMatch(/disabled=""[^>]*aria-label="Reload"/);
  });

  it("shows the bot's controls disabled and the address read-only", () => {
    const out = html(shown());
    expect(out).toMatch(/<input[^>]*aria-label="Browser address"[^>]*readOnly=""/);
    expect(out).toMatch(/disabled=""[^>]*aria-label="Reload"|aria-label="Reload"[^>]*disabled=""/);
  });

  it("shows a page dialog, answerable only by the user", () => {
    const dialog = { pageId: "p1", type: "confirm", message: "Delete it?", defaultValue: "" };
    expect(html(shown({ control: "bot", dialog }))).toContain("Take control to answer.");
    const mine = html(shown({ control: "human", dialog }));
    expect(mine).toContain("Delete it?");
    expect(mine).toContain("Cancel");
    expect(mine).toContain("OK");
  });

  it("says why it can't open, with a way to try again", () => {
    const out = html({ state: null, frame: null, error: "This bot's browser is off" });
    expect(out).toContain("This bot&#x27;s browser is off");
    expect(out).toContain("Try again");
  });
});

describe("native browser pane", () => {
  it("puts the page's own place where the picture would be, with the same toolbar", () => {
    const out = renderToStaticMarkup(
      <BrowserChrome view={{ state: state({ control: "human" }), frame: null, error: null }} botName="Scout" controls={controls} expanded={false} onToggleExpanded={() => {}} onRetry={() => {}} viewport={<div aria-label="Browser viewport" data-slot />} />,
    );
    expect(out).toContain('data-slot');
    expect(out).not.toContain("<img");
    for (const label of ["Browser address", "Back", "Forward", "Reload", "Expand browser", "Browser tab Example"]) expect(out).toContain(`aria-label="${label}"`);
    expect(out).toContain("Resume bot");
    expect(out).toContain('data-focus-return="control"');
  });

  it("says the browser needs the desktop app anywhere else", () => {
    const bot = { id: "b1", name: "Scout" } as Bot;
    expect(renderToStaticMarkup(<BrowserPanel bot={bot} active expanded={false} onToggleExpanded={() => {}} />)).toContain("The browser needs the Tjommie desktop app.");
  });
});
