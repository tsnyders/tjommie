import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import type { TaskPhase } from "../../protocol/src/projects.ts";
import type { StoredBot } from "./bots.ts";

export type TaskExecutionContext = Readonly<{ key: string; cwd: string; attemptId: string; revision: number; phase: TaskPhase | "discussion"; instructions: string }>;
export function executionKey(input: { botId: string; attemptId: string; revision: number; phase: TaskPhase | "discussion"; cwd: string; instructions: string }): string {
  return createHash("sha256").update(JSON.stringify([input.botId, input.attemptId, input.revision, input.phase,
    realpathSync.native(input.cwd), input.instructions])).digest("hex");
}
export function standaloneExecutionKey(bot: StoredBot): string {
  return createHash("sha256").update(JSON.stringify([realpathSync.native(bot.projectFolder ?? bot.homeFolder), bot.autonomy, bot.model, bot.instructions])).digest("hex");
}
