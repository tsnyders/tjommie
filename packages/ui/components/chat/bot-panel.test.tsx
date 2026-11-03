import type { Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BrowserCard } from "./bot-panel";

describe("Computer card", () => {
  it("offers remote opening and a visibility-gated preview without a desktop bridge", () => {
    const bot = { id: "synthetic", name: "Scout" } as Bot;
    const out = renderToStaticMarkup(<BrowserCard bot={bot} onOpen={() => {}} />);
    expect(out).toContain('aria-label="Open computer"');
    expect(out).toContain('data-slot="browser-thumbnail"');
    expect(out).not.toContain("Open Tjommie on that PC");
  });
});
