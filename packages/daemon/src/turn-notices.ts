import type { EventData } from "@tjommie/protocol";
import { getStoredBot } from "./bots.ts";
import type { Db } from "./db.ts";
import { getWorkItem, type WorkItem } from "./queue.ts";
import type { TurnResult } from "./runner.ts";

export function turnNotice(db: Db, item: WorkItem, result: TurnResult): EventData<"notify"> | null {
  if (item.source === "routine" || item.source === "bot_request" || result.outcome === "interrupted") return null;
  const chainId = result.outcome === "done" ? item.chainId ?? getWorkItem(db, item.id).chainId : null;
  if (chainId) {
    const row = db.prepare(`SELECT
      EXISTS(SELECT 1 FROM work_items WHERE chain_id = ? AND id <> ? AND status IN ('queued', 'running')) AS pending,
      EXISTS(SELECT 1 FROM work_items WHERE chain_id = ? AND routine_run_id IS NOT NULL) AS routine,
      EXISTS(SELECT 1 FROM exchanges WHERE chain_id = ? AND status = 'waiting') AS waiting`)
      .get(chainId, item.id, chainId, chainId) as { pending: number; routine: number; waiting: number };
    if (row.pending || row.routine || row.waiting) return null;
  }
  const bot = getStoredBot(db, item.botId);
  const room = db.prepare("SELECT name FROM conversations WHERE id = ? AND kind = 'room'").get(item.conversationId) as { name: string } | undefined;
  const roomId = room ? item.conversationId : undefined;
  if (result.outcome === "failed") return { kind: "failed", botId: bot.id, ...(roomId ? { roomId } : {}), title: `${bot.name} ran into a problem`, body: result.error ?? "Something went wrong." };
  return { kind: "done", botId: bot.id, ...(roomId ? { roomId } : {}), title: room ? `Done in ${room.name}` : `${bot.name} is done`, body: result.text ? Array.from(result.text).slice(0, 140).join("") : "" };
}
