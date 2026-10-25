import type { Autonomy, Bot, BotColor, BotPatch, BotShape, ParamsIn, Provider } from "@tjommie/protocol";
import { daysLabel, timesLabel } from "./format";

export type BotForm = {
  name: string;
  color: BotColor;
  shape: BotShape;
  instructions: string;
  provider: Provider;
  model: string;
  accountId: string | null; // null: the provider's default account
  autonomy: Autonomy;
  projectFolder: string | null;
  browser: boolean;
  canMessage: string[];
};

// A routine on a form: one the New bot screen creates with the bot, or the one the routine editor holds.
export type RoutineDraft = { name: string; prompt: string; days: number[]; times: string[]; timeLimitMinutes: number | null; enabled: boolean };

export const NEW_BOT: BotForm = {
  name: "",
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "claude",
  model: "sonnet",
  accountId: null,
  autonomy: "folder",
  projectFolder: null,
  browser: false,
  canMessage: [],
};

export const formFromBot = (bot: Bot): BotForm => ({
  name: bot.name,
  color: bot.color,
  shape: bot.shape,
  instructions: bot.instructions,
  provider: bot.provider,
  model: bot.model,
  accountId: bot.accountId ?? null,
  autonomy: bot.autonomy,
  projectFolder: bot.projectFolder,
  browser: bot.browser,
  canMessage: bot.canMessage,
});

// Why the name can't be saved yet, or null. The daemon checks again (name_taken).
export function nameProblem(name: string, bots: Pick<Bot, "id" | "name">[], selfId: string | null): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Give the bot a name.";
  if (trimmed.length > 40) return "Keep the name to 40 characters or fewer.";
  if (bots.some((bot) => bot.id !== selfId && bot.name.toLowerCase() === trimmed.toLowerCase())) return `There's already a bot called ${trimmed}.`;
  return null;
}

export const createInput = (form: BotForm): ParamsIn<"bots.create"> => ({ ...form, name: form.name.trim() });

export const sameSet = (a: string[], b: string[]): boolean => a.length === b.length && a.every((id) => b.includes(id));

// Only what the person changed, so a save never puts back a value that changed elsewhere since the form opened.
// Provider and model travel together: the daemon needs a model whenever the provider changes. The account travels on its own.
export function patchFor(bot: Bot, form: BotForm): BotPatch {
  const patch: BotPatch = {};
  const name = form.name.trim();
  if (name !== bot.name) patch.name = name;
  if (form.color !== bot.color) patch.color = form.color;
  if (form.shape !== bot.shape) patch.shape = form.shape;
  if (form.instructions !== bot.instructions) patch.instructions = form.instructions;
  if (form.provider !== bot.provider || form.model !== bot.model) {
    patch.provider = form.provider;
    patch.model = form.model;
  }
  if (form.accountId !== (bot.accountId ?? null)) patch.accountId = form.accountId;
  if (form.autonomy !== bot.autonomy) patch.autonomy = form.autonomy;
  if (form.projectFolder !== bot.projectFolder) patch.projectFolder = form.projectFolder;
  if (form.browser !== bot.browser) patch.browser = form.browser;
  if (!sameSet(form.canMessage, bot.canMessage)) patch.canMessage = form.canMessage;
  return patch;
}

// The Routines row's value: "None yet", "Weekdays 8:48 AM, 12:47 PM", "2 routines" or "1 paused".
export function routinesSummary(routines: Pick<RoutineDraft, "days" | "times" | "enabled">[]): string {
  if (routines.length === 0) return "None yet";
  const on = routines.filter((routine) => routine.enabled);
  if (on.length === 0) return `${routines.length} paused`;
  const days = daysLabel(on[0].days);
  if (on.every((routine) => daysLabel(routine.days) === days)) return `${days} ${timesLabel(on.flatMap((routine) => routine.times))}`;
  return `${on.length} routines`;
}

// The value of a row of bots to choose (Can message, a room's bots). Deleted bots don't count.
export function canMessageSummary(ids: string[], bots: Pick<Bot, "id">[]): string {
  const count = ids.filter((id) => bots.some((bot) => bot.id === id)).length;
  return count === 0 ? "Nobody yet" : `${count} ${count === 1 ? "bot" : "bots"}`;
}

export const AUTONOMY_LEVELS = [
  { value: "ask", label: "Always ask" },
  { value: "folder", label: "Own folder" },
  { value: "full", label: "Full auto" },
] as const satisfies readonly { value: Autonomy; label: string }[];

// The line under Autonomy. What "Own folder" doesn't protect is said plainly (spec 5.4).
export function autonomyHelp(autonomy: Autonomy, provider: Provider): string {
  if (autonomy === "ask") return "Always ask: asks before every change it makes.";
  if (autonomy === "full") return "Full auto: never asks. It can run any command and change any file your Windows account can.";
  return provider === "claude"
    ? "Own folder: works freely inside its own folder and asks before anything else. Claude bots also ask before each shell command."
    : "Own folder: works freely inside its own folder and asks before anything else. It can still read the rest of your user profile, and Codex bots can read each other's folders.";
}
