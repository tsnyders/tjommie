import type { Routine } from "@tjommie/protocol";
import type { RoutineDraft } from "./bot-form";
import { request } from "./connection";

const cleaned = (draft: RoutineDraft): RoutineDraft => ({ ...draft, name: draft.name.trim(), prompt: draft.prompt.trim() });

// Creates a routine for the bot, or saves the changes to one it already has. New routines get this PC's zone.
export async function saveRoutine(botId: string, routine: Routine | null, draft: RoutineDraft): Promise<void> {
  if (routine) await request("routines.update", { id: routine.id, patch: cleaned(draft) });
  else await request("routines.create", { botId, ...cleaned(draft) });
}

export async function removeRoutine(routine: Routine): Promise<void> {
  await request("routines.delete", { id: routine.id });
}
