"use client";

import type { ReactNode } from "react";
import type { Bot, Provider } from "@tjommie/protocol";
import type { BotForm } from "@/lib/bot-form";
import { cn } from "@/lib/utils";
import { FIELD, Label } from "../controls";
import { Identity } from "./identity";
import { ModelRow } from "./model-row";
import { AutonomyRow, BrowserRow, CanMessageRow, FolderRow } from "./rows";

// New bot and Bot settings share this column (WF3, WF8): the avatar and name, what the bot does, and its settings rows.
// The two screens handle routines differently, so each passes its own Routines row.
export function BotFormFields({ form, onChange, bots, selfId, saved, nameError, routinesRow }: {
  form: BotForm;
  onChange: (form: BotForm) => void;
  bots: Bot[];
  selfId: string | null;
  saved?: { provider: Provider; accountId: string | null }; // an existing bot's provider and account as saved
  nameError: string | null;
  routinesRow: ReactNode;
}) {
  return (
    <div className="flex flex-col">
      <Identity form={form} onChange={onChange} nameError={nameError} />
      <div className="mt-6 flex flex-col gap-1.5">
        <Label htmlFor="bot-instructions">What should this bot do?</Label>
        <textarea
          id="bot-instructions"
          value={form.instructions}
          maxLength={20_000}
          onChange={(event) => onChange({ ...form, instructions: event.target.value })}
          className={cn(FIELD, "max-h-[240px] min-h-[72px] resize-none py-3 [field-sizing:content]")}
        />
      </div>
      <div className="mt-6 flex flex-col gap-2">
        <ModelRow provider={form.provider} model={form.model} accountId={form.accountId} saved={saved} onChange={(value) => onChange({ ...form, ...value })} />
        <FolderRow projectFolder={form.projectFolder} onChange={(projectFolder) => onChange({ ...form, projectFolder })} />
        <AutonomyRow autonomy={form.autonomy} provider={form.provider} botName={form.name} onChange={(autonomy) => onChange({ ...form, autonomy })} />
        <BrowserRow browser={form.browser} onChange={(browser) => onChange({ ...form, browser })} />
        <CanMessageRow selfId={selfId} canMessage={form.canMessage} bots={bots} onChange={(canMessage) => onChange({ ...form, canMessage })} />
        {routinesRow}
      </div>
    </div>
  );
}
