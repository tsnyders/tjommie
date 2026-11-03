import { VAULT_HOST_BODY_BYTES, VaultHostJob, vaultHostResult } from "../../protocol/src/vault-host.ts";
import type { VaultHostCommand } from "../../protocol/src/vault.ts";

type Options = { port: number; token: string; execute(command: VaultHostCommand): Promise<unknown>; onError?: () => void };
async function readJob(response: Response): Promise<unknown> {
  const reader = response.body?.getReader(); if (!reader) throw new Error("Missing host command.");
  const chunks: Uint8Array[] = []; let size = 0;
  try {
    for (;;) { const { value, done } = await reader.read(); if (done) break; size += value.length; if (size > 16 * 1024) throw new Error("Oversized host command."); chunks.push(value); }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } finally { await reader.cancel().catch(() => {}); }
}
/** Main-process-only host; replies contain validated metadata or a fixed failure envelope. */
export function startVaultHost(options: Options): { close(): Promise<void> } {
  const controller = new AbortController(), url = `http://127.0.0.1:${options.port}/vault-host`, headers = { Authorization: `Bearer ${options.token}` }, active = new Set<Promise<void>>();
  let locking: Promise<void> | undefined, closing: Promise<void> | undefined;
  let globalBarrier = 0, channelEpoch = 0;
  const barriers = new Map<string, { sequence: number; expires: number }>();
  const lock = (): Promise<void> => {
    locking ??= options.execute({ controllerId: "local", action: { kind: "lock" } }).then((value) => { vaultHostResult({ kind: "lock" }, value); }).finally(() => { locking = undefined; });
    return locking;
  };
  const failClosed = async (): Promise<void> => { channelEpoch++; options.onError?.(); await lock().catch(() => {}); };
  const post = async (reply: { id: string; ok: false } | { id: string; ok: true; result: unknown }): Promise<void> => {
    const body = JSON.stringify(reply); if (Buffer.byteLength(body) > VAULT_HOST_BODY_BYTES) throw new Error("Oversized host response.");
    const response = await fetch(url, { method: "POST", headers, body, signal: controller.signal, redirect: "error" });
    if (response.status !== 204 && response.status !== 404) throw new Error("Host response was refused.");
  };
  const execute = async (job: VaultHostJob): Promise<void> => {
    let reply: { id: string; ok: false } | { id: string; ok: true; result: unknown };
    try { reply = { id: job.id, ok: true, result: vaultHostResult(job.command.action, await options.execute(job.command)) }; }
    catch { reply = { id: job.id, ok: false }; }
    if (!controller.signal.aborted) await post(reply);
  };
  const run = async (control: boolean): Promise<void> => {
    while (!controller.signal.aborted) {
      try {
        const epoch = channelEpoch;
        const response = await fetch(control ? `${url}?control=1` : url, { headers, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(30_000)]), redirect: "error" });
        if (response.status === 204) continue;
        if (!response.ok) throw new Error("Protected host unavailable.");
        const raw = await readJob(response), parsed = VaultHostJob.safeParse(raw);
        if (!parsed.success) { await failClosed(); continue; }
        const job = parsed.data, kind = job.command.action.kind;
        if (control && !["lock", "disconnect", "cancel"].includes(kind)) { await post({ id: job.id, ok: false }); await failClosed(); continue; }
        for (const [owner, barrier] of barriers) if (barrier.expires < Date.now()) barriers.delete(owner);
        if (kind === "lock") globalBarrier = Math.max(globalBarrier, job.sequence);
        else if (kind === "disconnect" || kind === "cancel") {
          if (barriers.size >= 2048 && !barriers.has(job.command.controllerId)) { await failClosed(); await post({ id: job.id, ok: false }); continue; }
          barriers.set(job.command.controllerId, { sequence: Math.max(barriers.get(job.command.controllerId)?.sequence ?? 0, job.sequence), expires: Date.now() + 60_000 });
        } else if (epoch !== channelEpoch || job.sequence <= globalBarrier || job.sequence <= (barriers.get(job.command.controllerId)?.sequence ?? 0)) { await post({ id: job.id, ok: false }); continue; }
        const task = execute(job); active.add(task);
        try { await task; } finally { active.delete(task); }
      } catch {
        if (controller.signal.aborted) break;
        await failClosed();
        await new Promise<void>((resolve) => {
          const finish = () => { clearTimeout(timer); controller.signal.removeEventListener("abort", finish); resolve(); }, timer = setTimeout(finish, 1000);
          controller.signal.addEventListener("abort", finish, { once: true });
        });
      }
    }
  };
  const running = Promise.all([run(false), run(true)]);
  return { close: () => closing ??= (async () => {
    controller.abort();
    const locked = lock();
    const results = await Promise.allSettled([locked, running]);
    await Promise.allSettled(active); // Aborted reply POSTs are expected after the native lock has completed.
    if (results.some((result) => result.status === "rejected")) throw new Error("The protected Logins host could not confirm a safe close.");
  })() };
}
