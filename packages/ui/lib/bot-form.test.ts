import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { autonomyHelp, canMessageSummary, createInput, formFromBot, nameProblem, NEW_BOT, patchFor, routinesSummary } from "./bot-form";

const bot: Bot = {
  id: "b1",
  name: "Pnet apply",
  color: "orange",
  shape: "cloud",
  instructions: "Apply to Full Stack roles.",
  provider: "claude",
  model: "sonnet",
  autonomy: "folder",
  homeFolder: "C:\\bots\\b1",
  projectFolder: null,
  browser: true,
  paused: false,
  canMessage: ["b2", "b3"],
  state: "idle",
  setupReason: null,
  limitedUntil: null,
  createdAt: "2026-09-21T08:00:00.000Z",
};

describe("the bot form", () => {
  it("sends only what the person changed", () => {
    const form = formFromBot(bot);
    expect(patchFor(bot, form)).toEqual({});
    expect(patchFor(bot, { ...form, name: " Job scout ", canMessage: ["b3", "b2"] })).toEqual({ name: "Job scout" });
    expect(patchFor(bot, { ...form, model: "haiku" })).toEqual({ provider: "claude", model: "haiku" });
    expect(patchFor(bot, { ...form, provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] })).toEqual({ provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] });
    expect(patchFor(bot, { ...form, projectFolder: "C:\\work", browser: false })).toEqual({ projectFolder: "C:\\work", browser: false });
  });

  it("checks the name and trims it for the daemon", () => {
    const bots = [bot, { ...bot, id: "b2", name: "CV Writer" }];
    expect(nameProblem("  ", bots, null)).toBe("Give the bot a name.");
    expect(nameProblem("x".repeat(41), bots, null)).toBe("Keep the name to 40 characters or fewer.");
    expect(nameProblem("cv writer", bots, null)).toBe("There's already a bot called cv writer.");
    expect(nameProblem("Pnet apply", bots, "b1")).toBeNull();
    expect(createInput({ ...NEW_BOT, name: " Night Shift " }).name).toBe("Night Shift");
  });

  it("sums up routines and Can message for their rows", () => {
    const weekdays = [1, 2, 3, 4, 5];
    expect(routinesSummary([])).toBe("None yet");
    expect(routinesSummary([{ days: weekdays, times: ["16:45"], enabled: true }, { days: weekdays, times: ["08:48", "12:47"], enabled: true }])).toBe("Weekdays 8:48 AM, 12:47 PM, 4:45 PM");
    expect(routinesSummary([{ days: weekdays, times: ["08:48"], enabled: true }, { days: [0], times: ["09:00"], enabled: true }])).toBe("2 routines");
    expect(routinesSummary([{ days: weekdays, times: ["08:48"], enabled: false }])).toBe("1 paused");
    expect(canMessageSummary(["b2", "gone"], [bot, { ...bot, id: "b2" }])).toBe("1 bot");
    expect(canMessageSummary([], [bot])).toBe("Nobody yet");
  });

  it("says what Own folder protects for each CLI", () => {
    expect(autonomyHelp("folder", "claude")).toContain("Claude bots also ask before each shell command.");
    expect(autonomyHelp("folder", "codex")).toContain("It can still read the rest of your user profile");
    expect(autonomyHelp("full", "codex")).toMatch(/^Full auto: never asks\./);
  });
});
