import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../app/settings/page.tsx", import.meta.url), "utf8");
const nav = source.slice(source.indexOf('<nav aria-label="Settings"'), source.indexOf(">", source.indexOf('<nav aria-label="Settings"')));

describe("the Settings list on a phone", () => {
  it("scrolls inside itself when the screen is too short for every section, so the last one can be reached", () => {
    expect(nav).toContain("max-sm:min-h-0");
    expect(nav).toContain("max-sm:overflow-y-auto");
    expect(nav).toContain("max-sm:flex-1");
  });

  it("keeps the section's own scroller and its test marker", () => {
    expect(source).toContain('data-testid="settings-scroll"');
  });
});
