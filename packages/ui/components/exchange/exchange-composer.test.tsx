import { readFileSync } from "node:fs";
import { TEAM_ROOM_ID, type Bot } from "@tjommie/protocol";
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
    expect(out).toContain('aria-label="Reply in Everyone"');
    expect(out).toContain('aria-label="Send"');
    expect(out).toContain("min-w-0 flex-1");
  });

  it("is unchanged on a desktop panel: the mentions sit in the box and don't shrink", () => {
    expect(out).toContain("sm:contents");
    expect(out).toContain("sm:shrink-0");
  });
});

describe("ExchangeComposer's reply", () => {
  const html = (names: string[]): string => renderToStaticMarkup(<ExchangeComposer bots={names.map((name, index) => bot(`b${index + 1}`, name))} />);

  it("is posted in Everyone, and says so", () => {
    expect(html(["Sales", "Marketing"])).toContain('placeholder="Ask both bots in Everyone"');
    expect(html(["Sales"])).toContain('placeholder="Ask Sales in Everyone"');
    expect(html([])).toContain('placeholder="Reply in Everyone"');
    // Once a reply is posted, which a static render can't reach, it's linked to Everyone.
    const source = readFileSync(new URL("./exchange-view.tsx", import.meta.url), "utf8");
    expect(source).toContain("Posted in Everyone.");
    expect(source).toContain("href={`/room/?id=${TEAM_ROOM_ID}`}");
    expect(TEAM_ROOM_ID).toBe("team");
  });
});
