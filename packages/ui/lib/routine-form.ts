import type { Routine, RoutineRun } from "@tjommie/protocol";
import type { RoutineDraft } from "./bot-form";
import { clock, dayName, duration } from "./format";

export const EMPTY_ROUTINE: RoutineDraft = { name: "", prompt: "", days: [1, 2, 3, 4, 5], times: ["09:00"], timeLimitMinutes: null, enabled: true };

export const draftFrom = (routine: Routine): RoutineDraft => ({
  name: routine.name,
  prompt: routine.prompt,
  days: routine.days,
  times: routine.times,
  timeLimitMinutes: routine.timeLimitMinutes,
  enabled: routine.enabled,
});

// The week as the editor shows it, Monday first. Days are 0 (Sunday) to 6.
export const WEEK = [
  { day: 1, letter: "M", name: "Monday" },
  { day: 2, letter: "T", name: "Tuesday" },
  { day: 3, letter: "W", name: "Wednesday" },
  { day: 4, letter: "T", name: "Thursday" },
  { day: 5, letter: "F", name: "Friday" },
  { day: 6, letter: "S", name: "Saturday" },
  { day: 0, letter: "S", name: "Sunday" },
] as const;

export const toggleDay = (days: number[], day: number): number[] => (days.includes(day) ? days.filter((each) => each !== day) : [...days, day].sort((a, b) => a - b));

// Adds an "HH:MM" time once, in order. Anything else leaves the times as they were.
export const addTime = (times: string[], time: string): string[] => (/^([01]\d|2[0-3]):[0-5]\d$/.test(time) && !times.includes(time) ? [...times, time].sort() : times);

// Why the routine can't be saved yet, or null. The daemon checks again.
export function routineProblem(draft: RoutineDraft): string | null {
  if (!draft.name.trim()) return "Give the routine a name.";
  if (draft.name.trim().length > 60) return "Keep the name to 60 characters or fewer.";
  if (!draft.prompt.trim()) return "Say what the bot should do.";
  if (draft.days.length === 0) return "Pick at least one day.";
  if (draft.times.length === 0) return "Add at least one time.";
  return null;
}

// Stop after (WF7). A limit set another way, like a bot's proposal, gets an option of its own.
export function stopAfterOptions(limit: number | null): { value: string; label: string }[] {
  const options = [
    { value: "none", label: "No limit" },
    { value: "60", label: "1 hour" },
    { value: "120", label: "2 hours" },
    { value: "180", label: "3 hours" },
  ];
  if (limit !== null && !options.some((option) => option.value === String(limit))) options.push({ value: String(limit), label: duration(limit * 60_000) });
  return options;
}

// The time zone new routines get: the daemon runs on this PC, so it's this PC's zone too.
export const localZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

// "Times are in South Africa Standard Time (Africa/Johannesburg)."
export function zoneLine(timeZone: string): string {
  const name = new Intl.DateTimeFormat("en-US", { timeZone, timeZoneName: "long" }).formatToParts(new Date()).find((part) => part.type === "timeZoneName")?.value;
  return name && name !== timeZone ? `Times are in ${name} (${timeZone}).` : `Times are in ${timeZone}.`;
}

export type RunLine = { icon: "done" | "stopped" | "failed" | "missed" | "skipped" | "running"; when: string; text: string; length: string | null };

// One row of Recent runs (WF7): when it was due, what happened, and how long it ran.
export function runLine(run: RoutineRun, now: Date = new Date()): RunLine {
  const when = `${dayName(run.dueAt, now)} ${clock(run.dueAt)}`;
  const length = run.startedAt && run.endedAt ? duration(Date.parse(run.endedAt) - Date.parse(run.startedAt)) : null;
  const note = run.note ? run.note.replace(/\.$/, "") : null;
  switch (run.status) {
    case "done":
      return { icon: "done", when, text: "Done", length };
    case "stopped":
      return { icon: "stopped", when, text: note ?? "Stopped", length };
    case "failed":
      return { icon: "failed", when, text: note ? `Failed: ${note}` : "Failed", length };
    case "missed":
      return { icon: "missed", when, text: "Missed: the PC was off", length: null };
    case "skipped":
      return { icon: "skipped", when, text: note ? `Skipped: ${note}` : "Skipped", length: null };
    case "queued":
      return { icon: "running", when, text: "Waiting to start", length: null };
    case "running":
      return { icon: "running", when, text: "Running", length: null };
  }
}
