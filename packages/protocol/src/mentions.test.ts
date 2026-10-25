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

  it("reads a room's mentions against every bot's name, and counts only the room's members", () => {
    const room = [bots[2]]; // only Bot is in the room
    expect(findMentions("@Bot 2 please help", bots, room)).toEqual([]); // Bot 2 isn't taken for Bot
    expect(findMentions("@bot please help", bots, room)).toEqual(["b"]);
    expect(findMentions("@Writer and @Bot", bots, room)).toEqual(["b"]);
    expect(findMentions("@everyone hi", bots, room)).toEqual(["b"]);
  });

  it("finds where each mention is, for highlighting", () => {
    expect(mentionRanges("@CV Writer add it. @everyone skip seniors.", ["Writer", "CV Writer"])).toEqual([
      { start: 0, end: 10, name: "CV Writer" },
      { start: 19, end: 28, name: null },
    ]);
    expect(mentionRanges("no mentions here", ["Writer"])).toEqual([]);
  });
});
