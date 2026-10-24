import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { TabBar } from "./tab-bar";

const path = vi.hoisted(() => ({ current: "/agents/" }));
vi.mock("next/navigation", () => ({ usePathname: () => path.current }));

const html = (pathname: string): string => {
  path.current = pathname;
  return renderToStaticMarkup(<TabBar />);
};

describe("TabBar (the Quiet pill)", () => {
  it("has the three destinations, each labelled, in a pill floating over the screen with nothing behind it", () => {
    const out = html("/agents/");
    expect(out).toContain('aria-label="Tjommie"');
    for (const name of ["Agents", "Team room", "Settings"]) expect(out).toContain(`aria-label="${name}"`);
    for (const shown of [">Agents<", ">Team<", ">Settings<"]) expect(out).toContain(shown);
    expect(out).toContain("rounded-full");
    expect(out).toContain("bg-tabbar"); // each phone style colours it
    expect(out).toContain('data-testid="phone-nav"');
    expect(out).toContain("fixed"); // over the screen, not a row of its own
    expect(out).toContain("pointer-events-none"); // the strip around the pill doesn't block the screen under it
    expect(out).toContain("sm:hidden"); // the rail serves wider windows
  });

  it("marks only the page's own tab, under the one capsule that slides between tabs", () => {
    const out = html("/room/");
    expect(out.match(/aria-current="page"/g)).toHaveLength(1);
    expect(out).toMatch(/aria-current="page"[^>]*aria-label="Team room"|aria-label="Team room"[^>]*aria-current="page"/);
    expect(out).toContain("translateX(100%)"); // the second tab
    expect(html("/agents/")).toContain("translateX(0%)");
    expect(html("/settings/accounts/")).toMatch(/aria-current="page"[^>]*aria-label="Settings"|aria-label="Settings"[^>]*aria-current="page"/);
  });

  it("is not there on chat or the bot forms, where a composer or an action footer sits at the bottom", () => {
    for (const pathname of ["/chat/", "/bots/new/", "/bots/settings/"]) expect(html(pathname)).toBe("");
  });
});
