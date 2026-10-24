import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ChatHeader } from "./chat-header";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: () => {}, back: () => {} }) }));
vi.mock("@/lib/connection", () => ({ daemon: vi.fn(), request: vi.fn() }));

const bot = { id: "b1", name: "A very long bot name of thirty-nine", color: "blue", shape: "circle", state: "waiting", paused: false, browser: false, homeFolder: "C:\\bots\\b1" } as unknown as Bot;
const html = (phone: boolean) => renderToStaticMarkup(<ChatHeader bot={bot} panelOpen={false} onTogglePanel={() => {}} onOpenBrowser={() => {}} phone={phone} />);

describe("ChatHeader on a phone", () => {
  const out = html(true);

  it("is Back, a title button that opens Details, and More, with no panel icon", () => {
    expect(out).toContain('aria-label="Back to Agents"');
    expect(out).toMatch(/aria-label="A very long bot name of thirty-nine, Waiting for you\. Show details"/);
    expect(out).toContain('aria-expanded="false"');
    expect(out).toContain('aria-label="More"');
    expect(out).not.toContain('aria-label="Bot panel"');
  });

  it("stacks the name over the state and cuts each short, so neither runs into More or the caption buttons", () => {
    expect(out).toContain("flex-col");
    expect(out.match(/truncate/g)?.length).toBeGreaterThanOrEqual(2); // the name and the state
    expect(out).toContain("overflow-hidden");
    expect(out).not.toMatch(/shrink-0[^"]*text-\[13px\][^"]*text-accent/); // the state no longer refuses to shrink
  });

  it("leaves the desktop header as it was: the panel icon, and the state beside the name", () => {
    const desktop = html(false);
    expect(desktop).toContain('aria-label="Bot panel"');
    expect(desktop).not.toContain("Show details");
  });
});
