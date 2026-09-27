"use client";

import { useState } from "react";
import { CircleAlert, Clock, Hourglass, Info, RotateCcw, Square, TimerOff } from "lucide-react";
import type { ErrorData, RoutineLabelData } from "@tjommie/protocol";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { Pill } from "../pill";

export function Timestamp({ label }: { label: string }) {
  return <div className="self-stretch py-2 text-center text-[12px] leading-[18px] text-caption">{label}</div>;
}

// The daemon's notices: "Stopped.", "Stopped at the time limit (3 hours).", a usage limit, and the like.
export function Notice({ text }: { text: string }) {
  const Icon = /time limit/i.test(text) ? TimerOff : /usage limit/i.test(text) ? Hourglass : /^stopped/i.test(text) ? Square : Info;
  return (
    <div className="flex items-center justify-center gap-2 self-stretch py-1 text-[12px] leading-[18px] text-secondary">
      <Icon className="size-3.5 shrink-0" aria-hidden />
      <span>{text}</span>
    </div>
  );
}

// An error, with Retry when the daemon can run that work again.
export function ErrorNotice({ text, retry }: { text: string; retry: ErrorData | null }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function run(target: ErrorData): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await daemon().call("chat.retry", { messageId: target.retryMessageId, botId: target.retryBotId });
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="flex w-full max-w-[600px] items-center gap-3 rounded-[12px] border border-card-stroke bg-card px-4 py-3">
      <CircleAlert className="size-4 shrink-0 text-secondary" aria-hidden />
      <div className="min-w-0 flex-1 text-[13px] leading-[18px]">
        <p className="text-text">{text}</p>
        {problem && <p className="text-danger">{problem}</p>}
      </div>
      {retry && (
        <Pill disabled={busy} onClick={() => void run(retry)}>
          <RotateCcw className="size-3.5" aria-hidden />
          Retry
        </Pill>
      )}
    </div>
  );
}

// "Routine: <name>" above a run. A missed or skipped run says why, with a gray clock.
export function RoutineLabel({ text, data }: { text: string; data: RoutineLabelData }) {
  const quiet = data.status === "missed" || data.status === "skipped";
  const note = (data.note ?? "").trim().replace(/\.$/, "");
  const why = data.status === "missed" ? "Missed, the PC was off" : data.status === "skipped" ? (note ? `Skipped: ${note}` : "Skipped") : null;
  return (
    <div className="flex items-center gap-2 py-1 text-[12px] font-medium leading-[18px] text-caption">
      <Clock className={cn("size-3.5 shrink-0", quiet ? "text-secondary" : "text-working")} aria-hidden />
      <span>
        {text}
        {why ? ` · ${why}` : ""}
      </span>
    </div>
  );
}
