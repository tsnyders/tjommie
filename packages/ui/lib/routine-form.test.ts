import type { RoutineRun } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { addTime, EMPTY_ROUTINE, routineProblem, runLine, stopAfterOptions, toggleDay, zoneLine } from "./routine-form";

const now = new Date(2026, 8, 27, 14, 0); // Sunday
const at = (day: number, hour: number, minute: number) => new Date(2026, 8, day, hour, minute).toISOString();
const run = (extra: Partial<RoutineRun>): RoutineRun => ({
  id: "run1",
  botId: "b1",
  routineId: "r1",
  dueAt: at(27, 8, 48),
  startedAt: null,
  endedAt: null,
  status: "done",
  note: null,
  ...extra,
});

describe("the routine editor", () => {
  it("edits days and times", () => {
    expect(toggleDay([1, 3], 2)).toEqual([1, 2, 3]);
    expect(toggleDay([1, 2, 3], 2)).toEqual([1, 3]);
    expect(addTime(["12:47"], "08:48")).toEqual(["08:48", "12:47"]);
    expect(addTime(["08:48"], "08:48")).toEqual(["08:48"]);
    expect(addTime(["08:48"], "24:00")).toEqual(["08:48"]);
  });

  it("says what's missing before saving, and offers a proposal's own time limit", () => {
    expect(routineProblem(EMPTY_ROUTINE)).toBe("Give the routine a name.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check" })).toBe("Say what the bot should do.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check", days: [] })).toBe("Pick at least one day.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check", times: [] })).toBe("Add at least one time.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check" })).toBeNull();
    expect(stopAfterOptions(180).map((option) => option.label)).toEqual(["No limit", "1 hour", "2 hours", "3 hours"]);
    expect(stopAfterOptions(90).at(-1)).toEqual({ value: "90", label: "1h 30m" });
    expect(zoneLine("Africa/Johannesburg")).toBe("Times are in South Africa Standard Time (Africa/Johannesburg).");
  });

  it("builds the Recent runs rows", () => {
    expect(runLine(run({ startedAt: at(27, 8, 48), endedAt: at(27, 11, 39) }), now)).toEqual({ icon: "done", when: "Today 8:48 AM", text: "Done", length: "2h 51m" });
    expect(runLine(run({ status: "stopped", note: "Stopped at the time limit.", startedAt: at(27, 8, 48), endedAt: at(27, 11, 48) }), now)).toEqual({
      icon: "stopped",
      when: "Today 8:48 AM",
      text: "Stopped at the time limit",
      length: "3h",
    });
    expect(runLine(run({ status: "failed", note: "Claude stopped unexpectedly" }), now)).toMatchObject({ icon: "failed", text: "Failed: Claude stopped unexpectedly", length: null });
    expect(runLine(run({ status: "missed", dueAt: at(25, 8, 48) }), now)).toEqual({ icon: "missed", when: "Friday 8:48 AM", text: "Missed: the PC was off", length: null });
    expect(runLine(run({ status: "skipped", note: "Pnet apply was paused." }), now)).toMatchObject({ icon: "skipped", text: "Skipped: Pnet apply was paused" });
  });
});
