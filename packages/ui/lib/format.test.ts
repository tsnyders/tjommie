import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { clock, dayName, fileSize, routineTitle, schedule, stamp } from "./format";
import { botName, errorText, modelLabel, stateLabel } from "./labels";

const now = new Date(2026, 8, 27, 14, 0); // Sunday 27 September 2026, 2:00 PM local time

describe("format", () => {
  it("writes times the way the wireframes do", () => {
    expect(clock(new Date(2026, 8, 27, 9, 14))).toBe("9:14 AM");
    expect(clock(new Date(2026, 8, 27, 17, 0))).toBe("5:00 PM");
    expect(stamp(new Date(2026, 8, 27, 9, 14), now)).toBe("Today 9:14 AM");
  });

  it("names days: today, yesterday, a weekday this week, else the date", () => {
    expect(dayName(new Date(2026, 8, 26, 22, 0), now)).toBe("Yesterday");
    expect(dayName(new Date(2026, 8, 22, 9, 0), now)).toBe("Tuesday");
    expect(dayName(new Date(2026, 8, 18, 9, 0), now)).toBe("18 September");
  });

  it("sizes files and describes routine schedules", () => {
    expect(fileSize(512)).toBe("512 bytes");
    expect(fileSize(184 * 1024)).toBe("184 KB");
    expect(fileSize(1.5 * 1024 * 1024)).toBe("1.5 MB");
    expect(schedule([1, 2, 3, 4, 5], ["08:48"])).toBe("Weekdays at 8:48 AM");
    expect(schedule([0, 1, 2, 3, 4, 5, 6], ["07:00", "17:00"])).toBe("Every day at 7:00 AM, 5:00 PM");
    expect(schedule([3, 1], ["09:00"])).toBe("Mon, Wed at 9:00 AM");
    expect(routineTitle("Pnet apply 9am", 180)).toBe("Pnet apply 9am (3h)");
    expect(routineTitle("Night digest", 90)).toBe("Night digest (90m)");
    expect(routineTitle("Night digest", null)).toBe("Night digest");
  });
});

describe("labels", () => {
  it("names bots, models and states, and says when a bot is gone", () => {
    const bots = [{ id: "b1", name: "Pnet apply" }] as Bot[];
    expect(botName(bots, "b1")).toBe("Pnet apply");
    expect(botName(bots, "gone")).toBe("A deleted bot");
    expect(modelLabel({ provider: "claude", model: "sonnet" })).toBe("Claude · Sonnet (latest)");
    expect(modelLabel({ provider: "codex", model: "gpt-6-astra" })).toBe("Codex · gpt-6-astra");
    expect(stateLabel({ state: "limited", limitedUntil: new Date(2026, 8, 27, 15, 45).toISOString() })).toBe("Limited until 3:45 PM");
    expect(stateLabel({ state: "waiting", limitedUntil: null })).toBe("Waiting for you");
    expect(errorText(new Error("bad_params: This bot's browser is off"))).toBe("This bot's browser is off");
  });
});
