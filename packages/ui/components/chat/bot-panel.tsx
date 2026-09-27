"use client";

import Link from "next/link";
import { useMemo, useState, type ReactNode } from "react";
import { Check, ChevronsRight, Clock, Folder, Loader, Pause, Settings, Shield, Sparkles, X, type LucideIcon } from "lucide-react";
import type { Bot, Message } from "@tjommie/protocol";
import { recentActivity } from "@/lib/activity";
import { daemon } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { clock, routineTitle, schedule } from "@/lib/format";
import { AUTONOMY_LINE, errorText, modelLabel, stateLabel } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { IconButton } from "../icon-button";

// The right panel beside a bot's chat (WF1 Phase 1). 16 px between cards, 24 px above a section label.
export function BotPanel({ bot, messages, onHide }: { bot: Bot; messages: Message[]; onHide: () => void }) {
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const steps = useMemo(() => recentActivity(messages), [messages]);
  return (
    <aside aria-label={`About ${bot.name}`} className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      {/* Windows draws the caption buttons over this bar's right end. */}
      <div className="drag flex h-11 shrink-0 items-center gap-1 px-3">
        <Link
          href={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`}
          aria-label="Bot settings"
          title="Bot settings"
          className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text"
        >
          <Settings className="size-4" aria-hidden />
        </Link>
        <IconButton label="Hide the bot panel" onClick={onHide}>
          <ChevronsRight aria-hidden />
        </IconButton>
      </div>
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-4">
        <Summary bot={bot} />
        {bot.browser && <BrowserCard bot={bot} />}
        <SectionLabel>Routines</SectionLabel>
        {routines.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No routines yet.</p>
        ) : (
          <ul className="flex flex-col gap-3">
            {routines.map((routine) => (
              <li key={routine.id} className="flex items-start gap-2.5">
                {routine.enabled ? <Clock className="mt-0.5 size-4 shrink-0 text-working" aria-hidden /> : <Pause className="mt-0.5 size-4 shrink-0 text-secondary" aria-hidden />}
                <div className="min-w-0">
                  <p className="truncate text-[13px] leading-[18px] text-title">{routineTitle(routine.name, routine.timeLimitMinutes)}</p>
                  <p className="text-[12px] leading-[18px] text-secondary">{routine.enabled ? schedule(routine.days, routine.times) : "Paused"}</p>
                </div>
              </li>
            ))}
          </ul>
        )}
        <SectionLabel>Recent activity</SectionLabel>
        {steps.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No recent activity.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {steps.map((step) => (
              <li key={step.key} className="flex items-center gap-2.5 text-[13px] leading-[18px]">
                {step.status === "running" ? (
                  <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
                ) : step.status === "done" ? (
                  <Check className="size-3.5 shrink-0 text-icon" aria-label="Done" />
                ) : (
                  <X className="size-3.5 shrink-0 text-secondary" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1 truncate text-body">{step.summary}</span>
                <span className="shrink-0 text-[12px] text-caption">{step.status === "running" ? "now" : clock(step.at)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="mb-2 mt-6 text-[13px] font-semibold leading-[18px] text-label">{children}</h3>;
}

function Row({ icon: Icon, children, action }: { icon: LucideIcon; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex items-start gap-2">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-icon" aria-hidden />
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </div>
  );
}

function OpenFolder({ path }: { path: string }) {
  return (
    <button type="button" onClick={() => void desktop()?.openFolder(path)} className="shrink-0 text-accent hover:underline">
      Open
    </button>
  );
}

function Summary({ bot }: { bot: Bot }) {
  return (
    <section className="mt-3 rounded-[12px] bg-card p-4">
      <div className="flex items-center gap-3">
        <BotAvatar color={bot.color} shape={bot.shape} size={40} className="shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-[17px] font-semibold text-title">{bot.name}</h2>
        {bot.state !== "idle" && (
          <span className="flex shrink-0 items-center gap-1.5 rounded-full bg-pill px-2.5 py-1 text-[12px] font-medium text-text">
            {bot.state === "working" && <Loader className="size-3 animate-spin" aria-hidden />}
            {stateLabel(bot)}
          </span>
        )}
      </div>
      <div className="mt-3 flex flex-col gap-2 pl-[52px] text-[13px] leading-[18px] text-body">
        <Row icon={Sparkles}>{modelLabel(bot)}</Row>
        <Row icon={Shield}>{AUTONOMY_LINE[bot.autonomy]}</Row>
        <Row icon={Folder} action={<OpenFolder path={bot.homeFolder} />}>
          Home folder
        </Row>
        {bot.projectFolder && (
          <Row icon={Folder} action={<OpenFolder path={bot.projectFolder} />}>
            <span className="block truncate">Project folder: {bot.projectFolder}</span>
          </Row>
        )}
      </div>
    </section>
  );
}

function BrowserCard({ bot }: { bot: Bot }) {
  const [problem, setProblem] = useState<string | null>(null);
  async function show(): Promise<void> {
    setProblem(null);
    try {
      await daemon().call("bots.showBrowser", { id: bot.id });
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <section className="mt-4 flex gap-3 rounded-[12px] bg-card p-3">
      <ChromeLogo />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[13px] font-semibold leading-[18px] text-title">Browser</h3>
          <button type="button" onClick={() => void show()} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            Show window
          </button>
        </div>
        <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">Its own Chrome window, separate from yours. Sites you sign in to there stay signed in.</p>
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
    </section>
  );
}

// The Chrome logo in its brand colors: an approved exception to the token palette.
function ChromeLogo() {
  return (
    <svg viewBox="0 0 48 48" width={20} height={20} aria-hidden="true" className="mt-0.5 shrink-0">
      <path d="M24 24 6.68 14A20 20 0 0 1 41.32 14Z" fill="#E33B2E" />
      <path d="M24 24 41.32 14A20 20 0 0 1 24 44Z" fill="#FBC116" />
      <path d="M24 24 24 44A20 20 0 0 1 6.68 14Z" fill="#229342" />
      <circle cx="24" cy="24" r="9.5" fill="#FFFFFF" />
      <circle cx="24" cy="24" r="7.5" fill="#1A73E8" />
    </svg>
  );
}
