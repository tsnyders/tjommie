import { Clock, Pause } from "lucide-react";
import { routineTitle, schedule } from "@/lib/format";

export type RoutineRow = { key: string; name: string; timeLimitMinutes: number | null; days: number[]; times: string[]; enabled: boolean };

// A bot's routines (the WF1 panel, and the Routines row on the bot forms). Each row opens the routine editor.
export function RoutineRows({ routines, onOpen }: { routines: RoutineRow[]; onOpen: (key: string) => void }) {
  if (routines.length === 0) return <p className="text-[13px] leading-[18px] text-secondary">No routines yet.</p>;
  return (
    <ul className="-mx-2 flex flex-col gap-1">
      {routines.map((routine) => (
        <li key={routine.key}>
          <button type="button" onClick={() => onOpen(routine.key)} className="flex w-full items-start gap-2.5 rounded-[10px] px-2 py-1.5 text-left hover:bg-highlight">
            {routine.enabled ? <Clock className="mt-0.5 size-4 shrink-0 text-working" aria-hidden /> : <Pause className="mt-0.5 size-4 shrink-0 text-secondary" aria-hidden />}
            <span className="min-w-0">
              <span className="block truncate text-[13px] leading-[18px] text-title">{routineTitle(routine.name, routine.timeLimitMinutes)}</span>
              <span className="block text-[12px] leading-[18px] text-secondary">{routine.enabled ? schedule(routine.days, routine.times) : "Paused"}</span>
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
