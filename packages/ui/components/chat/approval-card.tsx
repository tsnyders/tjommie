"use client";

import { useState } from "react";
import { Check, CircleAlert, X } from "lucide-react";
import type { ApprovalData, ApprovalDecision, Bot } from "@tjommie/protocol";
import { approvalDetail, approvalRule } from "@/lib/approval";
import { daemon } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { Pill } from "../pill";

// Answers given in this window, so an answered card can say "Allowed once" or "Always allowed".
const decided = new Map<string, ApprovalDecision>();

// A "Waiting for you" card. The buttons lock after one click; the card updates when the daemon records it.
export function ApprovalCard({ summary, data, bot }: { summary: string; data: ApprovalData; bot: Bot | undefined }) {
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  if (data.status !== "pending") return <AnsweredApproval summary={summary} data={data} />;
  const routine = data.kind === "routine";
  async function answer(decision: ApprovalDecision): Promise<void> {
    setBusy(true);
    setProblem(null);
    decided.set(data.approvalId, decision === "always" && !("rule" in data && data.rule) ? "once" : decision);
    try {
      await daemon().call("approvals.respond", { approvalId: data.approvalId, decision });
    } catch (error) {
      decided.delete(data.approvalId);
      setBusy(false);
      setProblem(errorText(error));
    }
  }
  return (
    <div role="group" aria-label="Waiting for you" className="w-full max-w-[600px] rounded-[14px] border border-waiting-stroke bg-waiting-fill p-4">
      <p className="flex items-center gap-2 text-[12px] font-semibold leading-[18px] text-accent">
        <CircleAlert className="size-3.5" aria-hidden />
        Waiting for you
      </p>
      <p className="mt-2 text-[14px] leading-[21px] text-text">{summary}</p>
      <p className={routine ? "mt-1 line-clamp-2 text-[12px] leading-[18px] text-secondary" : "mt-1 truncate text-[12px] leading-[18px] text-secondary"}>
        {routine ? `It will: ${data.detail}` : [data.tool, approvalDetail(data.detail)].filter(Boolean).join(" · ")}
      </p>
      <div className="mt-2 flex flex-wrap gap-2">
        {routine ? (
          <>
            <Pill primary disabled={busy} onClick={() => void answer("once")}>
              Add routine
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("deny")}>
              Dismiss
            </Pill>
          </>
        ) : (
          <>
            <Pill primary disabled={busy} onClick={() => void answer("once")}>
              Allow once
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("always")}>
              Always allow
            </Pill>
            <Pill disabled={busy} onClick={() => void answer("deny")}>
              Deny
            </Pill>
          </>
        )}
      </div>
      {data.kind !== "routine" && data.rule && bot && (
        <p className="mt-2 text-[12px] leading-[18px] text-placeholder">
          Always allow saves: {approvalRule(data.rule)}, for {bot.name} only
        </p>
      )}
      {problem && <p className="mt-2 text-[12px] leading-[18px] text-danger">{problem}</p>}
    </div>
  );
}

function AnsweredApproval({ summary, data }: { summary: string; data: ApprovalData }) {
  const allowed = data.status === "allowed";
  const decision = decided.get(data.approvalId);
  const label =
    data.kind === "routine"
      ? allowed
        ? "Routine added"
        : "Dismissed"
      : !allowed
        ? "Denied"
        : decision === "always"
          ? "Always allowed"
          : decision === "once"
            ? "Allowed once"
            : "Allowed";
  // A routine card's summary is the daemon's proposal ("Add a routine: Morning digest, ..."); once answered, the
  // label says what happened to it.
  const subject = data.kind === "routine" ? summary.replace(/^Add a routine: /, "") : summary;
  return (
    <div className="inline-flex max-w-full items-center gap-2 rounded-[10px] border border-divider bg-canvas px-3 py-2 text-[13px] leading-[18px] text-secondary">
      {allowed ? <Check className="size-3.5 shrink-0 text-working" aria-hidden /> : <X className="size-3.5 shrink-0" aria-hidden />}
      <span className="min-w-0 truncate">
        {label}: {subject}
      </span>
    </div>
  );
}
