import type { EventData } from "@tjommie/protocol";

type Notice = EventData<"notify">;

export const notificationRoute = (notice: Notice): string =>
  "roomId" in notice && notice.roomId ? `/room/?id=${encodeURIComponent(notice.roomId)}` : `/chat/?bot=${encodeURIComponent(notice.botId)}`;

// Retain delivery identities across notifier reconnects. A failed acknowledgement is retried without
// displaying the same live/startup notice twice during this desktop process's lifetime.
export function notificationDelivery() {
  const shown = new Set<string>();
  const pending = new Map<string, Promise<void>>();
  return (notice: Notice, show: (notice: Notice) => boolean | Promise<boolean>, acknowledge: (runId: string) => Promise<unknown>): Promise<void> => {
    const runId = notice.kind === "routine_failed" ? notice.runId : undefined;
    if (!runId) {
      return Promise.resolve().then(() => show(notice)).then(() => {});
    }
    const active = pending.get(runId);
    if (active) return active;
    const delivery = Promise.resolve().then(async () => {
      if (!shown.has(runId)) {
        if (!await show(notice)) return; // Failed or unsupported delivery remains pending in the daemon.
        shown.add(runId);
      }
      await acknowledge(runId);
    });
    pending.set(runId, delivery);
    void delivery.finally(() => {
      if (pending.get(runId) === delivery) pending.delete(runId);
    }).catch(() => {});
    return delivery;
  };
}
