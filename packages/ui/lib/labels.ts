import type { Autonomy, Bot } from "@tjommie/protocol";
import { clock } from "./format";

export const DELETED_BOT = "A deleted bot";

export function botName(bots: Pick<Bot, "id" | "name">[], id: string): string {
  return bots.find((bot) => bot.id === id)?.name ?? DELETED_BOT;
}

// The bot summary's autonomy line.
export const AUTONOMY_LINE: Record<Autonomy, string> = {
  ask: "Always ask: asks before every change",
  folder: "Own folder: works freely in its folder, asks before anything else",
  full: "Full auto: never asks",
};

// The empty chat's second sentence.
export const AUTONOMY_HINT: Record<Autonomy, string> = {
  ask: "It asks before every change.",
  folder: "It works in its own folder and asks before touching anything else.",
  full: "It works without asking first.",
};

const CLAUDE_MODELS: Record<string, string> = { sonnet: "Sonnet (latest)", opus: "Opus (latest)", haiku: "Haiku (latest)" };

export function modelLabel(bot: Pick<Bot, "provider" | "model">): string {
  return bot.provider === "claude" ? `Claude · ${CLAUDE_MODELS[bot.model] ?? bot.model}` : `Codex · ${bot.model}`;
}

export function stateLabel(bot: Pick<Bot, "state" | "limitedUntil">): string {
  switch (bot.state) {
    case "working":
      return "Working";
    case "waiting":
      return "Waiting for you";
    case "paused":
      return "Paused";
    case "limited":
      return bot.limitedUntil ? `Limited until ${clock(bot.limitedUntil)}` : "Limited";
    case "needs_setup":
      return "Needs setup";
    case "idle":
      return "Idle";
  }
}

// A daemon error as a sentence. The client's errors start with their code ("bad_params: ...").
export function errorText(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.replace(/^[a-z_]+: /, "");
}
