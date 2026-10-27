import type { EventData, PushKind } from "@tjommie/protocol";

type Target = { deviceId: string; token: string; kinds: PushKind[] | null };
type Result = { ok: boolean; unregistered: boolean };
type Stamp = { at: number; kind: PushKind; botId: string };
type Host = { pushTargets(): Target[]; clearPushToken(deviceId: string, token: string): void };
type Relay = { kinds(): readonly PushKind[]; push(tokens: string[], kind: PushKind, botId: string, roomId?: string): Promise<Result[]> };
type PushHandler = ((notify: EventData<"notify">) => void) & { failures: number };

export function startPush(options: { host: Host; relay: Relay }): PushHandler {
  const history = new Map<string, Stamp[]>();
  const send: PushHandler = (notify) => {
    if (!options.relay.kinds().includes(notify.kind)) return;
    const now = Date.now(), eligible: { target: Target; stamp: Stamp }[] = [];
    for (const target of options.host.pushTargets()) {
      if (target.kinds && !target.kinds.includes(notify.kind)) continue;
      const recent = (history.get(target.deviceId) ?? []).filter((entry) => now - entry.at < 3_600_000);
      if ((notify.kind !== "approval" && recent.filter((entry) => entry.kind !== "approval").length >= 30) ||
        recent.some((entry) => entry.kind === notify.kind && entry.botId === notify.botId && now - entry.at < 30_000)) continue;
      const stamp = { at: now, kind: notify.kind, botId: notify.botId };
      recent.push(stamp);
      history.set(target.deviceId, recent);
      eligible.push({ target, stamp });
    }
    for (let index = 0; index < eligible.length; index += 16) {
      const batch = eligible.slice(index, index + 16);
      void options.relay.push(batch.map(({ target }) => target.token), notify.kind, notify.botId, "roomId" in notify ? notify.roomId : undefined).then((results) => {
        if (results.length !== batch.length) { send.failures += batch.length; return; }
        results.forEach((result, position) => {
          if (!result.ok) send.failures++;
          if (result.unregistered) {
            try { options.host.clearPushToken(batch[position].target.deviceId, batch[position].target.token); }
            catch { send.failures++; }
          }
        });
      }).catch(() => {
        send.failures += batch.length;
        for (const { target, stamp } of batch) history.set(target.deviceId, (history.get(target.deviceId) ?? []).filter((entry) => entry !== stamp));
      });
    }
  };
  send.failures = 0;
  return send;
}
