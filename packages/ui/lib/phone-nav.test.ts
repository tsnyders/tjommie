import { describe, expect, it } from "vitest";
import { agentsTabName, homeTarget, showsTabBar, tabFor, TABS } from "./phone-nav";

describe("phone navigation", () => {
  it("shows the pill on the overview screens and not on chat or the bot forms", () => {
    for (const path of ["/", "/agents/", "/agents", "/room/", "/settings/", "/settings/accounts/"]) expect(showsTabBar(path)).toBe(true);
    for (const path of ["/chat/", "/bots/new/", "/bots/settings/", "/chat"]) expect(showsTabBar(path)).toBe(false);
  });

  it("has three labelled destinations", () => {
    expect(TABS.map((tab) => tab.label)).toEqual(["Agents", "Team room", "Settings"]);
    for (const tab of TABS) expect(tab.label).toContain(tab.short); // the name holds what shows
  });

  it("knows which tab a page belongs to, and that first run belongs to none", () => {
    expect([tabFor("/agents/"), tabFor("/room/"), tabFor("/settings/"), tabFor("/settings/x/")]).toEqual(["agents", "room", "settings", "settings"]);
    expect([tabFor("/"), tabFor("/chat/"), tabFor("/bots/settings/")]).toEqual([null, null, null]);
  });

  it("names the Agents tab for the bots waiting on the person", () => {
    expect(agentsTabName(0)).toBe("Agents");
    expect(agentsTabName(3)).toBe("Agents, 3 waiting for you");
  });

  it("sends a phone with bots to Agents, a desktop window to the first chat, and a first run nowhere", () => {
    expect(homeTarget("b 1", true)).toBe("/agents/");
    expect(homeTarget("b 1", false)).toBe("/chat/?bot=b%201");
    expect(homeTarget(undefined, true)).toBeNull();
    expect(homeTarget(undefined, false)).toBeNull();
  });
});
