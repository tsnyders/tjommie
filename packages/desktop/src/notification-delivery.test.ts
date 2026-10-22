import { describe, expect, it, vi } from "vitest";
import { notificationDelivery } from "./notification-delivery.ts";

const notice = { kind: "routine_failed", botId: "bot", runId: "run", routineId: "routine", title: "Routine failed", body: "Failure" } as const;

describe("routine notification delivery", () => {
  it("coalesces a live event and pending replay until acknowledgement completes", async () => {
    const deliver = notificationDelivery();
    const show = vi.fn(() => true);
    const acknowledged = Promise.withResolvers<void>();
    const ack = vi.fn(() => acknowledged.promise);
    const live = deliver(notice, show, ack);
    const replay = deliver(notice, show, ack);
    expect(live).toBe(replay);
    await vi.waitFor(() => expect(ack).toHaveBeenCalledWith("run"));
    expect(show).toHaveBeenCalledTimes(1);
    expect(ack).toHaveBeenCalledWith("run");
    acknowledged.resolve();
    await live;
  });

  it("retries acknowledgement after reconnect without repeating the popup", async () => {
    const deliver = notificationDelivery();
    const show = vi.fn(() => true);
    await expect(deliver(notice, show, async () => { throw new Error("Disconnected"); })).rejects.toThrow("Disconnected");
    const ack = vi.fn(async () => {});
    await deliver(notice, show, ack);
    expect(show).toHaveBeenCalledTimes(1);
    expect(ack).toHaveBeenCalledWith("run");
  });

  it("keeps unsupported delivery pending and allows it to be retried later", async () => {
    const deliver = notificationDelivery();
    const ack = vi.fn(async () => {});
    await deliver(notice, () => false, ack);
    expect(ack).not.toHaveBeenCalled();
    const show = vi.fn(() => true);
    await deliver(notice, show, ack);
    expect(show).toHaveBeenCalledOnce();
    expect(ack).toHaveBeenCalledOnce();
  });

  it("waits for asynchronous OS confirmation and retries a failed display", async () => {
    const deliver = notificationDelivery();
    const os = Promise.withResolvers<boolean>();
    const ack = vi.fn(async () => {});
    const delivery = deliver(notice, () => os.promise, ack);
    await Promise.resolve();
    expect(ack).not.toHaveBeenCalled();
    os.resolve(false);
    await delivery;
    expect(ack).not.toHaveBeenCalled();
    const confirmed = Promise.withResolvers<boolean>();
    const retry = deliver(notice, () => confirmed.promise, ack);
    await Promise.resolve();
    expect(ack).not.toHaveBeenCalled();
    confirmed.resolve(true);
    await retry;
    expect(ack).toHaveBeenCalledOnce();
  });

  it("does not coalesce distinct failures or acknowledgement-free notices", async () => {
    const deliver = notificationDelivery();
    const show = vi.fn(() => true);
    const ack = vi.fn(async () => {});
    await deliver(notice, show, ack);
    await deliver({ ...notice, runId: "another" }, show, ack);
    await deliver({ kind: "limit", botId: "bot", title: "Limit", body: "Wait" }, show, ack);
    expect(show).toHaveBeenCalledTimes(3);
    expect(ack).toHaveBeenCalledTimes(2);
  });
});
