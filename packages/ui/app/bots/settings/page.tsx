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
import { Pill } from "@/components/pill";
import { RoutineEditor } from "@/components/routines/routine-editor";
import { RoutineRows } from "@/components/routines/routine-rows";
import { formFromBot, nameProblem, patchFor, routinesSummary, type BotForm } from "@/lib/bot-form";
import { request } from "@/lib/connection";
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
      <header className="drag flex h-14 shrink-0 items-center justify-center">
        <div className="flex h-10 min-w-0 max-w-[calc(100%-352px)] items-center gap-2 rounded-full bg-chip px-3.5">
          <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
          <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
          <HeaderState bot={bot} />
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex gap-10 pb-10 pl-[120px] pr-6 pt-2 max-xl:pl-10">
          <div className="w-[640px] min-w-0 shrink">
            <BotFormFields
              form={form}
              onChange={setForm}
              bots={bots}
              selfId={bot.id}
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
          <aside className="ml-auto w-[340px] shrink-0">
            <h2 className="text-[13px] font-semibold leading-[18px] text-label">About this bot</h2>
            <About bot={bot} />
          </aside>
        </div>
      </div>
      <footer className="flex h-16 shrink-0 items-center gap-2 border-t border-card-stroke px-6">
        <button type="button" onClick={() => setDeleting(true)} className="mr-2 flex shrink-0 items-center gap-1.5 text-[14px] text-danger">
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
          <Pill onClick={() => router.push(chat)}>Cancel</Pill>
          <Pill primary disabled={busy || !dirty || nameError !== null} onClick={() => void run(() => request("bots.update", { id: bot.id, patch }), () => router.push(chat))}>
            Save changes
          </Pill>
        </span>
      </footer>
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${bot.name}?`}
        body="Its chat, routines and home folder (notes, inbox and Chrome profile) are deleted. Attached project folders aren't touched."
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

// About this bot (WF8): when it was made, its home folder, and the start of its notes.md.
function About({ bot }: { bot: Bot }) {
  const [notes, setNotes] = useState<string | null>(null);
  useEffect(() => {
    request("bots.notes", { id: bot.id }).then(
      ({ text }) => setNotes(text),
      () => setNotes(""),
    );
  }, [bot.id]);
  return (
    <section className="mt-3 rounded-[14px] border border-card-stroke bg-card p-4">
      <p className="text-[12px] leading-[18px] text-secondary">Created</p>
      <p className="mt-0.5 text-[14px] text-text">{longDate(bot.createdAt)}</p>
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">Home folder</p>
      <button type="button" onClick={() => void desktop()?.openFolder(bot.homeFolder)} className="mt-0.5 flex items-center gap-1.5 text-[14px] text-accent hover:underline">
        <Folder className="size-4" aria-hidden />
        Open folder
      </button>
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">Notes</p>
      <pre className="mt-1.5 max-h-[160px] overflow-hidden whitespace-pre-wrap rounded-[10px] bg-canvas p-3 font-sans text-[12px] leading-[18px] text-body [overflow-wrap:anywhere]">
        {notes === null ? "Loading" : notes.trim() || "No notes yet."}
      </pre>
      <button type="button" onClick={() => void desktop()?.openNotes(bot.homeFolder)} className="mt-3 text-[14px] text-accent hover:underline">
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
