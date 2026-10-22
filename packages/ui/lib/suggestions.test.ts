import { BotInput, RoutineInput } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { NEW_BOT } from "./bot-form";
import { SUGGESTIONS } from "./suggestions";

describe("Suggestions", () => {
  it("every suggestion is a valid bot with valid routines", () => {
    expect(SUGGESTIONS.map((suggestion) => suggestion.title)).toEqual(["Night Shift", "Inbox Triage", "Job hunter"]);
    for (const suggestion of SUGGESTIONS) {
      expect(suggestion.form.name).toBe(suggestion.title);
      expect(() => BotInput.parse({ ...NEW_BOT, ...suggestion.form })).not.toThrow();
      expect(suggestion.routines.length).toBeGreaterThan(0);
      for (const routine of suggestion.routines) expect(() => RoutineInput.parse({ botId: "b1", ...routine })).not.toThrow();
    }
  });
});
