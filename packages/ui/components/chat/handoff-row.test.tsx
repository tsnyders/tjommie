import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HandoffRow } from "./handoff-row";

// Outside the app, Link drops the trailing slash the static export keeps (trailingSlash), so it's put back to compare.
const href = (html: string): string | undefined => html.match(/href="([^"]*)"/)?.[1].replace("/room?", "/room/?");

describe("a room answer's row in a bot's chat", () => {
  it("says which room the bot answered in, and opens that room", () => {
    const out = renderToStaticMarkup(<HandoffRow body="Answered in Growth" data={{ roomMessageId: "m1", roomId: "r 1" }} bots={[]} conversationId="b1" />);
    expect(href(out)).toBe("/room/?id=r%201");
    expect(out).toContain(">Answered in Growth<");
  });

  it("opens Everyone for a row from before there were other rooms", () => {
    const out = renderToStaticMarkup(<HandoffRow body="Answered in the Team room" data={{ roomMessageId: "m1" }} bots={[]} conversationId="b1" />);
    expect(href(out)).toBe("/room/?id=team");
    expect(out).toContain(">Answered in the Team room<");
  });
});
