import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ExchangeComposer } from "./exchange-view";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: vi.fn(), closeConversation: vi.fn(), exchangePage: vi.fn(), loadConversation: vi.fn(), retryConversation: vi.fn() }));

const bot = (id: string, name: string): Bot => ({ id, name, color: "blue", shape: "circle" }) as Bot;
const LONG = "A very long bot name of thirty-nine";

describe("ExchangeComposer on a phone", () => {
  const out = renderToStaticMarkup(<ExchangeComposer bots={[bot("b1", LONG), bot("b2", "ResearchScout")]} />);

  it("puts the mentions on a row of their own that wraps, so they can't push the box off the screen", () => {
    expect(out).toContain("max-sm:basis-full");
    expect(out).toContain("max-sm:flex-wrap");
    expect(out).toContain("max-sm:max-w-full"); // a chip is at most as wide as the row
  });

  it("cuts a long name short but keeps its full name for a screen reader and a tooltip", () => {
    expect(out).toContain(`title="@${LONG}"`);
    expect(out).toContain(`@${LONG}`);
    expect(out).toContain("@ResearchScout");
    expect(out).toContain("truncate");
  });

  it("keeps the input and Send in the box, the input taking what room is left", () => {
    expect(out).toContain('aria-label="Reply in Team room"');
    expect(out).toContain('aria-label="Send"');
    expect(out).toContain("min-w-0 flex-1");
  });

  it("is unchanged on a desktop panel: the mentions sit in the box and don't shrink", () => {
    expect(out).toContain("sm:contents");
    expect(out).toContain("sm:shrink-0");
  });
});
