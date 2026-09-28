"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot } from "@tjommie/protocol";
import { BotAvatar } from "@/components/bot-avatar";
import { BotFormFields } from "@/components/bot-form/bot-form";
import { RoutinesRow } from "@/components/bot-form/rows";
import { Pill } from "@/components/pill";
import { RoutineEditor } from "@/components/routines/routine-editor";
import { RoutineRows } from "@/components/routines/routine-rows";
import { createInput, nameProblem, NEW_BOT, routinesSummary, type BotForm, type RoutineDraft } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { useApp } from "@/lib/store";
import { SUGGESTIONS, type Suggestion } from "@/lib/suggestions";

const chatOf = (bot: Bot): string => `/chat/?bot=${encodeURIComponent(bot.id)}`;

// New bot (WF3): the form, Suggestions at the right, and Create bot. Routines added here are created with the bot.
export default function NewBotPage() {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const [form, setForm] = useState<BotForm>(NEW_BOT);
  const [routines, setRoutines] = useState<RoutineDraft[]>([]);
  const [editing, setEditing] = useState<number | "new" | null>(null);
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [created, setCreated] = useState<Bot | null>(null);
  // Not while the bot is being made, or once it exists: it joins the list, and its name mustn't clash with itself.
  const nameError = touched && !busy && !created ? nameProblem(form.name, bots, null) : null;

  function applySuggestion(suggestion: Suggestion): void {
    setForm({ ...NEW_BOT, ...suggestion.form });
    setRoutines(suggestion.routines);
    setTouched(false);
    setProblem(null);
  }

  async function create(): Promise<void> {
    setTouched(true);
    if (busy || nameProblem(form.name, bots, null)) return;
    setBusy(true);
    setProblem(null);
    let bot: Bot;
    try {
      bot = await request("bots.create", createInput(form));
    } catch (error) {
      setProblem(errorText(error));
      setBusy(false);
      return;
    }
    const failed: string[] = [];
    for (const routine of routines) {
      try {
        await request("routines.create", { botId: bot.id, ...routine, name: routine.name.trim(), prompt: routine.prompt.trim() });
      } catch (error) {
        failed.push(`${routine.name} (${errorText(error)})`);
      }
    }
    if (failed.length === 0) {
      router.push(chatOf(bot));
      return;
    }
    // The bot exists now, so this screen mustn't offer to create it again.
    setCreated(bot);
    setProblem(`${bot.name} was created, but not these routines: ${failed.join(", ")}.`);
    setBusy(false);
  }

  const draft = editing === "new" ? EMPTY_ROUTINE : editing === null ? null : routines[editing];
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag flex h-14 shrink-0 items-center justify-center">
        <div className="flex h-10 items-center gap-2 rounded-full bg-chip px-3.5">
          <BotAvatar color={form.color} shape={form.shape} size={24} />
          <h1 className="text-[15px] font-semibold text-title">New bot</h1>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex gap-10 pb-10 pl-[120px] pr-6 pt-2 max-xl:pl-10">
          <div className="w-[640px] min-w-0 shrink">
            <BotFormFields
              form={form}
              onChange={setForm}
              bots={bots}
              selfId={null}
              nameError={nameError}
              routinesRow={
                <RoutinesRow summary={routinesSummary(routines)}>
                  <RoutineRows routines={routines.map((routine, index) => ({ ...routine, key: String(index) }))} onOpen={(key) => setEditing(Number(key))} />
                  <button type="button" onClick={() => setEditing("new")} className="mt-2 text-[13px] text-accent hover:underline">
                    Add routine
                  </button>
                </RoutinesRow>
              }
            />
          </div>
          <aside className="ml-auto w-[340px] shrink-0">
            <h2 className="text-[13px] font-semibold leading-[18px] text-label">Suggestions</h2>
            <div className="mt-3 flex flex-col gap-3">
              {SUGGESTIONS.map((suggestion) => (
                <button
                  key={suggestion.title}
                  type="button"
                  onClick={() => applySuggestion(suggestion)}
                  className="press flex items-center gap-4 rounded-[14px] border border-card-stroke bg-card p-4 text-left hover:border-input-stroke"
                >
                  <BotAvatar color={suggestion.form.color} shape={suggestion.form.shape} size={40} className="shrink-0" />
                  <span>
                    <span className="block text-[15px] font-semibold leading-[20px] text-title">{suggestion.title}</span>
                    <span className="mt-0.5 block text-[13px] leading-[18px] text-secondary">{suggestion.description}</span>
                  </span>
                </button>
              ))}
            </div>
          </aside>
        </div>
      </div>
      <footer className="flex h-16 shrink-0 items-center justify-end gap-2 border-t border-card-stroke px-6">
        {problem && (
          <p role="alert" className="mr-auto min-w-0 text-[13px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        {created ? (
          <Pill primary onClick={() => router.push(chatOf(created))}>
            Open {created.name}
          </Pill>
        ) : (
          <>
            <Pill onClick={() => router.back()}>Cancel</Pill>
            <Pill primary disabled={busy} onClick={() => void create()}>
              Create bot
            </Pill>
          </>
        )}
      </footer>
      {draft && (
        <RoutineEditor
          key={String(editing)}
          title={editing === "new" ? "New routine" : "Edit routine"}
          initial={draft}
          timeZone={localZone()}
          onClose={() => setEditing(null)}
          onSave={async (next) => {
            setRoutines(editing === "new" ? [...routines, next] : routines.map((routine, index) => (index === editing ? next : routine)));
          }}
          onDelete={
            editing === "new"
              ? undefined
              : async () => {
                  setRoutines(routines.filter((_, index) => index !== editing));
                }
          }
        />
      )}
    </div>
  );
}
