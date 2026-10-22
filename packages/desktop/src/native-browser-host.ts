import { z } from "zod";
import { NativeBrowserCommand, type NativeBrowserCommand as Command } from "../../protocol/src/native-browser.ts";

const Job = z.object({
  id: z.string().uuid(),
  bot: z.object({ id: z.string().min(1).max(256), name: z.string().max(1000), homeFolder: z.string().min(1).max(32_768), projectFolder: z.string().max(32_768).nullable().optional() }),
  command: NativeBrowserCommand,
});
type Options = {
  port: number;
  token: string;
  execute(bot: z.infer<typeof Job>["bot"], command: Command): Promise<unknown>;
  onError?: () => void;
};

// This client is main-process-only. Its capability is not the renderer's daemon token.
export function startNativeBrowserHost(options: Options): { close(): Promise<void> } {
  const controller = new AbortController();
  const url = `http://127.0.0.1:${options.port}/browser-host`;
  const headers = { Authorization: `Bearer ${options.token}` };
  const active = new Set<Promise<void>>();
  const execute = async (job: z.infer<typeof Job>): Promise<void> => {
    let answer: { id: string; ok: boolean; result?: unknown; error?: string };
    try { answer = { id: job.id, ok: true, result: await options.execute(job.bot, job.command) }; }
    catch (error) { answer = { id: job.id, ok: false, error: error instanceof Error ? error.message.slice(0, 2000) : "The native browser operation failed." }; }
    if (!controller.signal.aborted) await fetch(url, { method: "POST", headers, body: JSON.stringify(answer), signal: controller.signal, redirect: "error" });
  };
  const run = async (control = false): Promise<void> => {
    while (!controller.signal.aborted) {
      try {
        if (!control && active.size >= 4) { await Promise.race(active); continue; }
        const response = await fetch(control ? `${url}?control=1` : url, { headers, signal: controller.signal, redirect: "error" });
        if (response.status === 204) continue;
        if (!response.ok) throw new Error("Native host connection failed.");
        const raw: unknown = await response.json();
        const parsed = Job.safeParse(raw);
        if (!parsed.success) {
          if (typeof raw === "object" && raw !== null && "id" in raw && typeof raw.id === "string") {
            await fetch(url, { method: "POST", headers, body: JSON.stringify({ id: raw.id, ok: false, error: "The desktop browser command did not match the installed host protocol." }), signal: controller.signal, redirect: "error" });
          }
          continue;
        }
        const job = parsed.data;
        const task = execute(job).catch(() => { if (!controller.signal.aborted) options.onError?.(); });
        active.add(task);
        void task.then(() => active.delete(task));
      } catch {
        if (controller.signal.aborted) break;
        options.onError?.();
        await new Promise<void>((resolve) => {
          const finish = (): void => { clearTimeout(timer); controller.signal.removeEventListener("abort", finish); resolve(); };
          const timer = setTimeout(finish, 1000);
          controller.signal.addEventListener("abort", finish, { once: true });
        });
      }
    }
  };
  const running = Promise.all([run(), run(true)]);
  return { close: async () => { controller.abort(); await running; await Promise.allSettled(active); } };
}
