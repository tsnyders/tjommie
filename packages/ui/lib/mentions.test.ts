import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { insertMention, mentionOptions, typedMention } from "./mentions";

const bots = [{ name: "Careers24 scout" }, { name: "Chief of Staff" }, { name: "CV Writer" }] as Bot[];

describe("typing a mention", () => {
  it("finds the @ being typed, and ignores email addresses", () => {
    expect(typedMention("Hi @Ca", 6)).toEqual({ start: 3, query: "Ca" });
    expect(typedMention("@Chief of", 9)).toEqual({ start: 0, query: "Chief of" });
    expect(typedMention("mail me@wr", 10)).toBeNull();
    expect(typedMention("no mention", 10)).toBeNull();
    expect(typedMention("@a\nb", 4)).toBeNull();
  });

  it("offers the bots whose names start with it, then everyone", () => {
    expect(mentionOptions("ca", bots).map((option) => option.name)).toEqual(["Careers24 scout"]);
    expect(mentionOptions("c", bots).map((option) => option.name)).toEqual(["Careers24 scout", "Chief of Staff", "CV Writer"]);
    expect(mentionOptions("", bots).map((option) => option.name)).toEqual(["Careers24 scout", "Chief of Staff", "CV Writer", "everyone"]);
    expect(mentionOptions("ev", bots)).toEqual([{ name: "everyone", bot: null }]);
    expect(mentionOptions("zz", bots)).toEqual([]);
  });

  it("puts the whole mention in, with a space after it", () => {
    expect(insertMention("Hi @Ca please", 3, 6, "Careers24 scout")).toEqual({ text: "Hi @Careers24 scout  please", caret: 20 });
  });
});
