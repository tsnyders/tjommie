import { readFileSync } from "node:fs";
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
const bots = [{ id: "b1" }, { id: "b2" }, { id: "b3" }]; // the bots there are

describe("the bot form", () => {
  it("sends only what the person changed", () => {
    const form = formFromBot(bot);
    expect(patchFor(bot, form, bots)).toEqual({});
    expect(patchFor(bot, { ...form, name: " Job scout ", canMessage: ["b3", "b2"] }, bots)).toEqual({ name: "Job scout" });
    expect(patchFor(bot, { ...form, model: "haiku" }, bots)).toEqual({ provider: "claude", model: "haiku" });
    expect(patchFor(bot, { ...form, provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] }, bots)).toEqual({ provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] });
    expect(patchFor(bot, { ...form, projectFolder: "C:\\work", browser: false }, bots)).toEqual({ projectFolder: "C:\\work", browser: false });
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

describe("saving bot settings", () => {
  it("sends only what was changed in the form, so a change made elsewhere meanwhile isn't put back", () => {
    // Renamed "Job scout" elsewhere after the form opened; here only the instructions changed, so the name isn't sent.
    expect(patchFor(bot, { ...formFromBot(bot), instructions: "Apply to React roles." }, bots)).toEqual({ instructions: "Apply to React roles." });
  });

  it("measures the edit from the bot as the form opened, not from the bot as it is now", () => {
    const source = readFileSync(new URL("../app/bots/settings/page.tsx", import.meta.url), "utf8");
    expect(source).toContain("const [opened] = useState(bot);");
    expect(source).toContain("patchFor(opened, form, bots)");
  });

  it("never sends a Can message bot deleted since the form opened, and doesn't count it as a change", () => {
    const left = bots.filter((each) => each.id !== "b2"); // b2 deleted elsewhere
    expect(patchFor(bot, formFromBot(bot), left)).toEqual({}); // Save stays off
    expect(patchFor(bot, { ...formFromBot(bot), canMessage: ["b2", "b3", "b4"] }, [...left, { id: "b4" }])).toEqual({ canMessage: ["b3", "b4"] });
    expect(patchFor(bot, { ...formFromBot(bot), canMessage: ["b2"] }, left)).toEqual({ canMessage: [] }); // b3 taken out here
  });
});

describe("the account", () => {
  const saved = { id: "b1", provider: "claude", model: "sonnet", accountId: "a1", name: "Scout", color: "blue", shape: "circle", instructions: "", autonomy: "folder", projectFolder: null, browser: true, canMessage: [] } as unknown as Bot;

  it("starts a new bot on the provider's default account, and sends the pin when creating", () => {
    expect(NEW_BOT.accountId).toBeNull();
    expect(createInput({ ...NEW_BOT, name: " Scout ", accountId: "a2" }).accountId).toBe("a2");
  });

  it("reads a bot's pin, and sends it only when it changed, apart from the provider and model", () => {
    const form = formFromBot(saved);
    expect(form.accountId).toBe("a1");
    expect(patchFor(saved, form, bots)).toEqual({});
    expect(patchFor(saved, { ...form, accountId: "a2" }, bots)).toEqual({ accountId: "a2" });
    expect(patchFor(saved, { ...form, accountId: null }, bots)).toEqual({ accountId: null });
    expect(patchFor(saved, { ...form, provider: "codex", model: "gpt", accountId: "c1" }, bots)).toEqual({ provider: "codex", model: "gpt", accountId: "c1" });
    expect(patchFor({ ...saved, accountId: undefined } as Bot, formFromBot({ ...saved, accountId: undefined } as Bot), bots)).toEqual({});
  });
});
