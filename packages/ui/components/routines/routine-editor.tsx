"use client";

import { useEffect, useRef, useState } from "react";
import { Check, CircleAlert, CircleDashed, Globe, Loader, Pause, Plus, TimerOff, Trash2, X, type LucideIcon } from "lucide-react";
import type { RoutineRun } from "@tjommie/protocol";
import type { RoutineDraft } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { timesLabel } from "@/lib/format";
import { errorText } from "@/lib/labels";
import { addTime, routineProblem, runLine, stopAfterOptions, toggleDay, WEEK, zoneLine, type RunLine } from "@/lib/routine-form";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { FIELD, Label, Segmented, Switch } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";

const RUN_ICONS: Record<RunLine["icon"], LucideIcon> = { done: Check, stopped: TimerOff, failed: CircleAlert, missed: CircleDashed, skipped: Pause, running: Loader };
// WF7 dims a run by how it went: Done reads brightest, Stopped and Failed less, Missed and Skipped least.
const RUN_TONES: Record<RunLine["icon"], { icon: string; text: string }> = {
  done: { icon: "text-working", text: "text-body" },
  running: { icon: "animate-spin text-icon", text: "text-body" },
  stopped: { icon: "text-secondary", text: "text-secondary" },
  failed: { icon: "text-secondary", text: "text-secondary" },
  missed: { icon: "text-placeholder", text: "text-placeholder" },
  skipped: { icon: "text-placeholder", text: "text-placeholder" },
};

type Props = {
  title: string;
  initial: RoutineDraft;
  timeZone: string;
  runsFor?: string; // a saved routine's id: shows its Recent runs
  onClose: () => void;
  onSave: (draft: RoutineDraft) => Promise<void>;
  onDelete?: () => Promise<void>;
};

// The routine editor (WF7): a 560 px dialog. It stays open until Save or Delete works, and says why when they don't.
export function RoutineEditor({ title, initial, timeZone, runsFor, onClose, onSave, onDelete }: Props) {
  const [draft, setDraft] = useState(initial);
  const [adding, setAdding] = useState(false);
  const addAnother = useRef(false); // Enter added a time: Add time takes focus back for the next one
  const [runs, setRuns] = useState<RoutineRun[] | "failed" | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const nameField = useRef<HTMLInputElement>(null);
  const latestRun = useApp((state) => (runsFor ? state.latestRuns[runsFor] : undefined));

  // Recent runs load when the editor opens, and again each time the daemon reports a run of this routine.
  useEffect(() => {
    if (!runsFor) return;
    let live = true;
    request("routines.runs", { routineId: runsFor }).then(
      (list) => {
        if (live) setRuns(list);
      },
      () => {
        if (live) setRuns("failed");
      },
    );
    return () => {
      live = false;
    };
  }, [runsFor, latestRun]);

  async function act(action: () => Promise<void>): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      onClose();
    } catch (error) {
      setProblem(errorText(error));
      setBusy(false);
    }
  }

  function save(): void {
    const missing = routineProblem(draft);
    if (missing) setProblem(missing);
    else void act(() => onSave(draft));
  }

  const limit = draft.timeLimitMinutes === null ? "none" : String(draft.timeLimitMinutes);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={title}
      width={560}
      className="px-6 pb-3"
      closable
      initialFocus={nameField}
      actions={
        <span className="flex items-center gap-2 text-[13px] text-secondary">
          On
          <Switch label="Routine on" checked={draft.enabled} onChange={(enabled) => setDraft({ ...draft, enabled })} />
        </span>
      }
    >
      {/* WF7's rhythm: 16 px from each control to the next label, 4 px from a label to its control. */}
      <div className="mt-4 flex flex-col gap-1">
        <Label htmlFor="routine-name">Name</Label>
        <input id="routine-name" ref={nameField} value={draft.name} maxLength={60} onChange={(event) => setDraft({ ...draft, name: event.target.value })} className={cn(FIELD, "h-11")} />
      </div>
      <div className="mt-4 flex flex-col gap-1">
        <Label htmlFor="routine-prompt">What to do</Label>
        <textarea
          id="routine-prompt"
          value={draft.prompt}
          maxLength={10_000}
          onChange={(event) => setDraft({ ...draft, prompt: event.target.value })}
          className={cn(FIELD, "max-h-[200px] min-h-[96px] resize-none py-3 [field-sizing:content]")}
        />
      </div>
      <p className="mt-4 text-[13px] leading-[18px] text-secondary">Days</p>
      <div role="group" aria-label="Days" className="mt-1 flex gap-2">
        {WEEK.map(({ day, letter, name }) => {
          const on = draft.days.includes(day);
          return (
            <button
              key={day}
              type="button"
              aria-label={name}
              aria-pressed={on}
              onClick={() => setDraft({ ...draft, days: toggleDay(draft.days, day) })}
              className={cn("flex h-8 w-10 items-center justify-center rounded-full text-[13px]", on ? "bg-user-bubble font-semibold text-user-text" : "bg-pill text-secondary")}
            >
              {letter}
            </button>
          );
        })}
      </div>
      <p className="mt-4 text-[13px] leading-[18px] text-secondary">Times</p>
      <div className="mt-1 flex flex-wrap items-center gap-2">
        {draft.times.map((time) => (
          <span key={time} className="inline-flex h-8 items-center gap-1.5 rounded-full bg-pill pl-3 pr-1.5 text-[14px] text-text">
            {timesLabel([time])}
            <button
              type="button"
              aria-label={`Remove ${timesLabel([time])}`}
              onClick={() => setDraft({ ...draft, times: draft.times.filter((each) => each !== time) })}
              className="flex size-5 items-center justify-center rounded-full text-secondary hover:text-text"
            >
              <X className="size-3" aria-hidden />
            </button>
          </span>
        ))}
        {adding ? (
          // A time field has a value as soon as each part has a digit (typing 08:4 gives 08:04), so the time is added
          // when the field loses focus (Enter does that too), never while it's typed.
          <input
            type="time"
            autoFocus
            aria-label="New time"
            onKeyDown={(event) => {
              if (event.key !== "Enter") return;
              event.preventDefault(); // or the same Enter goes on to press Add time once it has focus
              addAnother.current = true;
              event.currentTarget.blur();
            }}
            onBlur={(event) => {
              setDraft({ ...draft, times: addTime(draft.times, event.target.value) });
              setAdding(false);
            }}
            className="h-8 rounded-full border border-selected bg-canvas px-3 text-[14px] text-text"
          />
        ) : (
          <button
            ref={(button) => {
              if (!button || !addAnother.current) return;
              addAnother.current = false;
              button.focus();
            }}
            type="button"
            onClick={() => setAdding(true)}
            className="inline-flex h-8 items-center gap-1.5 rounded-full border border-selected bg-canvas px-3 text-[14px] text-body hover:bg-pill"
          >
            <Plus className="size-3.5" aria-hidden />
            Add time
          </button>
        )}
      </div>
      <p className="mt-3 flex items-center gap-1.5 text-[12px] leading-[18px] text-secondary">
        <Globe className="size-3.5 shrink-0" aria-hidden />
        {zoneLine(timeZone)}
      </p>
      <p className="mt-3 text-[13px] leading-[18px] text-secondary">Stop after</p>
      <div className="mt-1">
        <Segmented
          label="Stop after"
          value={limit}
          options={stopAfterOptions(draft.timeLimitMinutes)}
          onChange={(value) => setDraft({ ...draft, timeLimitMinutes: value === "none" ? null : Number(value) })}
        />
      </div>
      {runsFor && (
        <div className="mt-5 border-t border-divider pt-4">
          <div className="flex items-baseline justify-between">
            <h3 className="text-[13px] font-semibold leading-[18px] text-label">Recent runs</h3>
            <span className="text-[12px] leading-[18px] text-caption">Last 20 runs are kept</span>
          </div>
          {runs === "failed" && <p className="mt-2 text-[13px] leading-[18px] text-secondary">Couldn&apos;t load the runs.</p>}
          {Array.isArray(runs) && runs.length === 0 && <p className="mt-2 text-[13px] leading-[18px] text-secondary">No runs yet.</p>}
          {Array.isArray(runs) && runs.length > 0 && (
            <ul className="mt-2 flex flex-col">
              {runs.map((run) => {
                const line = runLine(run);
                const Icon = RUN_ICONS[line.icon];
                const tone = RUN_TONES[line.icon];
                return (
                  <li key={run.id} className="flex h-[26px] items-center gap-2.5 text-[13px]">
                    <Icon className={cn("size-3.5 shrink-0", tone.icon)} aria-hidden />
                    <span className="w-[142px] shrink-0 text-body">{line.when}</span>
                    <span className={cn("min-w-0 flex-1 truncate", tone.text)}>{line.text}</span>
                    {line.length && <span className="shrink-0 text-[12px] text-caption">{line.length}</span>}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
      <div className="mt-5 flex items-center gap-3 border-t border-divider pt-3">
        {onDelete && (
          <button type="button" disabled={busy} onClick={() => void act(onDelete)} className="flex shrink-0 items-center gap-1.5 text-[14px] text-danger disabled:opacity-50">
            <Trash2 className="size-4" aria-hidden />
            Delete routine
          </button>
        )}
        {problem && (
          <p role="alert" className="min-w-0 flex-1 text-right text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <span className="ml-auto flex shrink-0 gap-2">
          <Pill onClick={onClose}>Cancel</Pill>
          <Pill primary disabled={busy} onClick={save}>
            Save
          </Pill>
        </span>
      </div>
    </Dialog>
  );
}
