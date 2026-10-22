"use client";

import { useMemo, useState, type ReactNode } from "react";
import { Check, Folder, Globe, Loader, Shield, Sparkles, X, type LucideIcon } from "lucide-react";
import type { Bot, Message, Routine } from "@tjommie/protocol";
import { recentActivity } from "@/lib/activity";
import { desktop } from "@/lib/desktop";
import { clock } from "@/lib/format";
import { AUTONOMY_LINE, modelLabel, stateLabel } from "@/lib/labels";
import { draftFrom, EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { removeRoutine, saveRoutine } from "@/lib/routines";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { PanelBar } from "../panel-bar";
import { RoutineEditor } from "../routines/routine-editor";
import { RoutineRows } from "../routines/routine-rows";

// The right panel beside a bot's chat (WF1 Phase 1). 16 px between cards, 24 px above a section label.
export function BotPanel({ bot, messages, onHide, onOpenBrowser }: { bot: Bot; messages: Message[]; onHide: () => void; onOpenBrowser: () => void }) {
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const steps = useMemo(() => recentActivity(messages), [messages]);
  return (
    <aside aria-label={`About ${bot.name}`} className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      <PanelBar settingsHref={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`} settingsLabel="Bot settings" hideLabel="Hide the bot panel" onHide={onHide} />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-4">
        <Summary bot={bot} />
        {bot.browser && <BrowserCard onOpen={onOpenBrowser} />}
        <PanelRoutines bot={bot} routines={routines} />
        <SectionLabel>Recent activity</SectionLabel>
        {steps.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No recent activity.</p>
        ) : (
          <ul className="flex flex-col gap-3.5">
            {steps.map((step) => (
              <li key={step.key} className="flex items-center gap-2.5 text-[13px] leading-[18px]">
                {step.status === "running" ? (
                  <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
                ) : step.status === "done" ? (
                  <Check className="size-3.5 shrink-0 text-working" aria-label="Done" />
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

// Routines with Add (WF1 Phase 1). Rows and Add open the routine editor; saves go straight to the daemon.
function PanelRoutines({ bot, routines }: { bot: Bot; routines: Routine[] }) {
  const [editing, setEditing] = useState<Routine | "new" | null>(null);
  const routine = editing === "new" ? null : editing;
  return (
    <>
      <div className="mb-2.5 mt-6 flex items-center justify-between">
        <h3 className="text-[13px] font-semibold leading-[18px] text-label">Routines</h3>
        <button type="button" aria-label="Add a routine" onClick={() => setEditing("new")} className="text-[13px] leading-[18px] text-accent hover:underline">
          Add
        </button>
      </div>
      <RoutineRows routines={routines.map((each) => ({ ...each, key: each.id }))} onOpen={(key) => setEditing(routines.find((each) => each.id === key) ?? null)} />
      {editing && (
        <RoutineEditor
          key={routine?.id ?? "new"}
          title={routine ? "Edit routine" : "New routine"}
          initial={routine ? draftFrom(routine) : EMPTY_ROUTINE}
          timeZone={routine?.timeZone ?? localZone()}
          runsFor={routine?.id}
          onClose={() => setEditing(null)}
          onSave={(draft) => saveRoutine(bot.id, routine, draft)}
          onDelete={routine ? () => removeRoutine(routine) : undefined}
        />
      )}
    </>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="mb-3 mt-6 text-[13px] font-semibold leading-[18px] text-label">{children}</h3>;
}

function Row({ icon: Icon, children, action }: { icon: LucideIcon; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex items-start gap-1.5">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-secondary" aria-hidden />
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </div>
  );
}

function OpenFolder({ path, label }: { path: string; label: string }) {
  return (
    <button type="button" aria-label={label} onClick={() => void desktop()?.openFolder(path)} className="shrink-0 text-accent hover:underline">
      Open
    </button>
  );
}

// The avatar has a column of its own; the name and the rows under it share the other (WF1 Phase 1).
function Summary({ bot }: { bot: Bot }) {
  return (
    <section className="mt-3 flex gap-3 rounded-[12px] bg-card p-4">
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className="shrink-0" />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-3">
          <h2 className="min-w-0 flex-1 truncate text-[17px] font-semibold leading-[26px] text-title">{bot.name}</h2>
          {bot.state !== "idle" && (
            <span className="flex h-[22px] shrink-0 items-center gap-1 rounded-full bg-pill px-2 text-[12px] text-text">
              {bot.state === "working" && <Loader className="size-3 animate-spin" aria-hidden />}
              {stateLabel(bot)}
            </span>
          )}
        </div>
        <div className="mt-1 flex flex-col gap-1.5 text-[13px] leading-[18px] text-body">
          <Row icon={Sparkles}>{modelLabel(bot)}</Row>
          <Row icon={Shield}>{AUTONOMY_LINE[bot.autonomy]}</Row>
          <Row icon={Folder} action={<OpenFolder path={bot.homeFolder} label="Open the home folder" />}>
            Home folder
          </Row>
          {bot.projectFolder && (
            <Row icon={Folder} action={<OpenFolder path={bot.projectFolder} label="Open the project folder" />}>
              <span className="block truncate">Project folder: {bot.projectFolder}</span>
            </Row>
          )}
        </div>
      </div>
    </section>
  );
}

function BrowserCard({ onOpen }: { onOpen: () => void }) {
  return (
    <section className="mt-4 flex items-center gap-3 rounded-[12px] bg-card p-3">
      <Globe className="size-5 shrink-0 text-icon" aria-hidden />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[13px] font-semibold leading-[18px] text-title">Browser</h3>
          <button type="button" aria-label="Open browser" onClick={onOpen} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            Open
          </button>
        </div>
        <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">Built into Tjommie, with its own sign-ins. Watch the bot use it, or take over.</p>
      </div>
    </section>
  );
}
