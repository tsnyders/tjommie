"use client";

import { Suspense, useEffect, useMemo, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { Folder, Pause, Play, Trash2 } from "lucide-react";
import type { Bot, Routine } from "@tjommie/protocol";
import { BotAvatar } from "@/components/bot-avatar";
import { BotFormFields } from "@/components/bot-form/bot-form";
import { RoutinesRow } from "@/components/bot-form/rows";
import { HeaderState } from "@/components/chat/chat-header";
import { ConfirmDialog } from "@/components/dialog";
import { BackLink } from "@/components/back-link";
import { BAR_PILL, Pill } from "@/components/pill";
import { RoutineEditor } from "@/components/routines/routine-editor";
import { RoutineRows } from "@/components/routines/routine-rows";
import { formFromBot, nameProblem, patchFor, routinesSummary, type BotForm } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { phoneCaptionClear } from "@/lib/native-capability";
import { cn } from "@/lib/utils";
import { desktop } from "@/lib/desktop";
import { longDate } from "@/lib/format";
import { errorText } from "@/lib/labels";
import { draftFrom, EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { removeRoutine, saveRoutine } from "@/lib/routines";
import { useApp } from "@/lib/store";

function Settings() {
  const botId = useSearchParams().get("bot") ?? "";
  const bots = useApp((state) => state.bots);
  const bot = useMemo(() => bots.find((candidate) => candidate.id === botId), [bots, botId]);
  // key: each bot's settings start from that bot.
  return bot ? <BotSettings key={bot.id} bot={bot} bots={bots} /> : <GoHome />;
}

// An unknown or deleted bot has no settings.
function GoHome() {
  const router = useRouter();
  useEffect(() => {
    router.replace("/");
  }, [router]);
  return null;
}

// Bot settings (WF8): the New bot form filled from the bot, About this bot, and the action bar.
function BotSettings({ bot, bots }: { bot: Bot; bots: Bot[] }) {
  const router = useRouter();
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const [form, setForm] = useState<BotForm>(() => formFromBot(bot));
  const [editing, setEditing] = useState<Routine | "new" | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const chat = `/chat/?bot=${encodeURIComponent(bot.id)}`;
  const nameError = nameProblem(form.name, bots, bot.id);
  const patch = patchFor(bot, form);
  const dirty = Object.keys(patch).length > 0;

  async function run(action: () => Promise<unknown>, then?: () => void): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      then?.();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  const routine = editing === "new" ? null : editing;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* The chip is centered over the form column, as on New bot (WF3). */}
      <header className={cn("drag flex h-14 shrink-0 items-center pl-[120px] pr-6 max-xl:pl-10 max-sm:gap-1 max-sm:pl-1", phoneCaptionClear())}>
        <BackLink href="/agents/" label="Back" history />
        <div className="flex w-[640px] min-w-0 shrink justify-center max-sm:w-auto max-sm:flex-1 max-sm:justify-start">
          <div className="flex h-10 min-w-0 items-center gap-2 rounded-full bg-chip px-3.5">
            <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
            <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
            <HeaderState bot={bot} />
          </div>
        </div>
      </header>
      <div data-testid="bot-form-scroll" className="min-h-0 flex-1 overflow-y-auto">
        {/* When the form and About this bot don't fit side by side, About moves under the form. 532 px holds the avatar tile and the color row. */}
        <div className="flex flex-wrap justify-between gap-10 pb-6 pl-[120px] pr-6 pt-2 max-xl:pl-10 max-sm:gap-6 max-sm:pl-4 max-sm:pr-4">
          <div className="w-[640px] min-w-[532px] shrink max-sm:w-full max-sm:min-w-0">
            <BotFormFields
              form={form}
              onChange={setForm}
              bots={bots}
              selfId={bot.id}
              saved={{ provider: bot.provider, accountId: bot.accountId ?? null }}
              nameError={nameError}
              routinesRow={
                <RoutinesRow summary={routinesSummary(routines)}>
                  <RoutineRows routines={routines.map((each) => ({ ...each, key: each.id }))} onOpen={(key) => setEditing(routines.find((each) => each.id === key) ?? null)} />
                  <button type="button" onClick={() => setEditing("new")} className="mt-2 text-[13px] text-accent hover:underline">
                    Add routine
                  </button>
                </RoutinesRow>
              }
            />
          </div>
          <aside className="w-[340px] shrink-0 max-sm:w-full">
            <h2 className="text-[13px] font-semibold leading-[18px] text-label">About this bot</h2>
            <About bot={bot} />
          </aside>
        </div>
      </div>
      <footer className="flex h-16 shrink-0 items-center gap-2 border-t border-card-stroke px-6 max-sm:h-auto max-sm:flex-wrap max-sm:px-4 max-sm:py-3">
        <button type="button" disabled={busy} onClick={() => setDeleting(true)} className="mr-4 flex shrink-0 items-center gap-1.5 text-[14px] text-danger disabled:opacity-50">
          <Trash2 className="size-4" aria-hidden />
          Delete bot
        </button>
        <Pill disabled={busy} onClick={() => void run(() => request("bots.pause", { id: bot.id, paused: !bot.paused }))}>
          {bot.paused ? <Play className="size-3.5" aria-hidden /> : <Pause className="size-3.5" aria-hidden />}
          {bot.paused ? "Resume bot" : "Pause bot"}
        </Pill>
        {problem && (
          <p role="alert" className="min-w-0 truncate text-[13px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <span className="ml-auto flex shrink-0 gap-2">
          <Pill className={BAR_PILL} onClick={() => router.push(chat)}>
            Cancel
          </Pill>
          <Pill
            primary
            className={BAR_PILL}
            disabled={busy || !dirty || nameError !== null}
            onClick={() => void run(() => request("bots.update", { id: bot.id, patch }), () => router.push(chat))}
          >
            Save changes
          </Pill>
        </span>
      </footer>
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${bot.name}?`}
        body="Its chat, routines and home folder (notes, inbox and browser profile) are deleted. Attached project folders aren't touched."
        confirm={
          <>
            <Trash2 className="size-4" aria-hidden />
            Delete bot
          </>
        }
        danger
        onConfirm={() => {
          setDeleting(false);
          void run(() => request("bots.delete", { id: bot.id }), () => router.replace("/"));
        }}
      />
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
    </div>
  );
}

// About this bot (WF8): when it was made, its home folder, and the start of its notes.md. A missing notes.md comes
// back empty, so a failed call means the notes couldn't be read (or Tjommie isn't connected).
function About({ bot }: { bot: Bot }) {
  const [notes, setNotes] = useState<{ text: string } | "failed" | null>(null);
  useEffect(() => {
    request("bots.notes", { id: bot.id }).then(setNotes, () => setNotes("failed"));
  }, [bot.id]);
  return (
    <section className="mt-1.5 rounded-[14px] border border-card-stroke bg-card p-4">
      <p className="text-[12px] leading-[18px] text-secondary">Created</p>
      <p className="mt-0.5 text-[14px] text-text">{longDate(bot.createdAt)}</p>
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">Home folder</p>
      <button type="button" onClick={() => void desktop()?.openFolder(bot.homeFolder)} className="mt-0.5 flex items-center gap-1.5 text-[14px] text-accent hover:underline">
        <Folder className="size-4" aria-hidden />
        Open folder
      </button>
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">Notes</p>
      <pre className="mt-1.5 max-h-[160px] overflow-hidden whitespace-pre-wrap rounded-[10px] bg-canvas p-3 font-sans text-[12px] leading-[18px] text-body [overflow-wrap:anywhere]">
        {notes === null ? "Loading" : notes === "failed" ? "Couldn't read the notes." : notes.text.trim() || "No notes yet."}
      </pre>
      <button type="button" onClick={() => void desktop()?.openNotes(bot.homeFolder)} className="mt-2.5 text-[14px] text-accent hover:underline">
        Open notes.md
      </button>
    </section>
  );
}

export default function BotSettingsPage() {
  return (
    <Suspense>
      <Settings />
    </Suspense>
  );
}
