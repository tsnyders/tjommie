import { describe, expect, it } from "vitest";
import { findMentions, mentionRanges } from "./mentions.ts";

describe("mentions", () => {
  const bots = [
    { id: "w", name: "Writer" },
    { id: "cv", name: "CV Writer" },
    { id: "b", name: "Bot" },
    { id: "b2", name: "Bot 2" },
  ];

  it("matches whole names, longest first, and ignores email addresses (the UI and the daemon read mentions the same way)", () => {
    expect(findMentions("@CV Writer can you help?", bots)).toEqual(["cv"]);
    expect(findMentions("@writer and @cv writer!", bots)).toEqual(["w", "cv"]);
    expect(findMentions("@Bot 2 and @Bot 2 again", bots)).toEqual(["b2"]);
    expect(findMentions("mail me@writer.com or @Writers", bots)).toEqual([]);
    expect(findMentions("mail me_@writer.com or me+@writer.com or @Writer_extra", bots)).toEqual([]);
    expect(findMentions("@everyone stop", bots)).toEqual(["w", "cv", "b", "b2"]);
    expect(findMentions("@everyone stop", [...bots, { id: "all", name: "Everyone" }])).toEqual(["w", "cv", "b", "b2", "all"]);
  });

  it("finds where each mention is, for highlighting", () => {
    expect(mentionRanges("@CV Writer add it. @everyone skip seniors.", ["Writer", "CV Writer"])).toEqual([
      { start: 0, end: 10, name: "CV Writer" },
      { start: 19, end: 28, name: null },
    ]);
    expect(mentionRanges("no mentions here", ["Writer"])).toEqual([]);
  });
});
