"use client";

import { useState } from "react";
import { Check, ChevronDown, ChevronUp, ListChecks, Loader, X } from "lucide-react";
import type { ActivityItem } from "@tjommie/protocol";

// One line per turn of tool work; it expands to one row per step.
export function ActivityRow({ summary, items }: { summary: string; items: ActivityItem[] }) {
  const [open, setOpen] = useState(false);
  const header = (
    <button
      type="button"
      onClick={() => setOpen(!open)}
      aria-expanded={open}
      className="inline-flex h-8 max-w-full items-center gap-2 rounded-[10px] px-3 text-left text-[13px] text-secondary hover:text-body"
    >
      <ListChecks className="size-3.5 shrink-0" aria-hidden />
      <span className="min-w-0 truncate">{summary}</span>
      {open ? <ChevronUp className="size-3.5 shrink-0" aria-hidden /> : <ChevronDown className="size-3.5 shrink-0" aria-hidden />}
    </button>
  );
  if (!open) return header;
  return (
    <div className="w-full max-w-[600px] rounded-[12px] border border-card-stroke py-1">
      {header}
      <ul className="px-3 pb-2">
        {items.map((item) => (
          <li key={item.id} className="flex items-center gap-2 py-1 text-[13px] text-body">
            {item.status === "done" ? (
              <Check className="size-3.5 shrink-0 text-working" aria-label="Done" />
            ) : item.status === "running" ? (
              <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
            ) : (
              <X className="size-3.5 shrink-0 text-secondary" aria-label="Failed" />
            )}
            <span className="min-w-0 truncate">{item.summary}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
