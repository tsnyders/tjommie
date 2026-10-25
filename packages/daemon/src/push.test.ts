import { afterEach, expect, it, vi } from "vitest";
import type { EventData, PushKind } from "@tjommie/protocol";
import { startPush } from "./push.ts";

const notice: EventData<"notify"> = { kind: "approval", botId: "bot-1", title: "private content", body: "private detail" };
type Result = { ok: boolean; unregistered: boolean };
function setup(count = 1) {
  const targets = Array.from({ length: count }, (_, index) => ({ deviceId: `device-${index}`, token: `synthetic-token-${index}` }));
  const cleared: { deviceId: string; token: string }[] = [];
  const calls: { tokens: string[]; kind: PushKind; botId: string }[] = [];
  let results: Result[] | null = null;
  let reject = false;
  const sender = startPush({
    host: { pushTargets: () => targets, clearPushToken: (deviceId, token) => { cleared.push({ deviceId, token }); } },
    relay: { push: async (tokens, kind, botId) => { calls.push({ tokens, kind, botId }); if (reject) throw new Error("synthetic-token-0"); return results && calls.length === 1 ? results : tokens.map(() => ({ ok: true, unregistered: false })); } },
  });
  return { sender, calls, cleared, setResults: (value: Result[]) => { results = value; }, fail: () => { reject = true; }, recover: () => { reject = false; } };
}
afterEach(() => vi.useRealTimers());

it("coalesces by device, kind and bot, then enforces 30 per device per hour", async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  const { sender, calls } = setup();
  sender(notice); sender(notice);
  await vi.waitFor(() => expect(calls).toHaveLength(1));
  vi.setSystemTime(30_000);
  for (let i = 0; i < 29; i++) sender({ ...notice, botId: `bot-${i}` });
  await vi.waitFor(() => expect(calls).toHaveLength(30));
  sender({ ...notice, botId: "overflow" });
  expect(calls).toHaveLength(30);
  vi.setSystemTime(3_600_001);
  sender(notice);
  await vi.waitFor(() => expect(calls).toHaveLength(31));
});

it("batches at 16, clears unregistered tokens, and counts failures without logging content", async () => {
  const logs = (["log", "info", "warn", "error"] as const).map((method) => vi.spyOn(console, method).mockImplementation(() => {}));
  try {
    const { sender, calls, cleared, setResults, fail } = setup(17);
    setResults(Array.from({ length: 16 }, (_, index) => ({ ok: index !== 0, unregistered: index === 0 })));
    sender(notice);
    await vi.waitFor(() => expect(calls).toHaveLength(2));
    await vi.waitFor(() => expect(cleared).toEqual([{ deviceId: "device-0", token: "synthetic-token-0" }]));
    expect(calls.map((call) => call.tokens.length)).toEqual([16, 1]);
    expect(calls[0]).toMatchObject({ kind: "approval", botId: "bot-1" });
    expect(JSON.stringify(calls)).not.toContain("private detail");
    expect(sender.failures).toBe(1);
    fail(); sender({ ...notice, kind: "limit" });
    await vi.waitFor(() => expect(sender.failures).toBe(18));
    expect(JSON.stringify(logs.flatMap((spy) => spy.mock.calls))).not.toContain("synthetic-token");
  } finally { logs.forEach((spy) => spy.mockRestore()); }
});

it("restores rate budget when an offline push is rejected", async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  const { sender, calls, fail, recover } = setup();
  fail(); sender(notice);
  await vi.waitFor(() => expect(sender.failures).toBe(1));
  recover(); vi.setSystemTime(10_000); sender(notice);
  expect(calls).toHaveLength(2);
  for (let index = 0; index < 30; index++) {
    fail(); sender({ ...notice, botId: `failed-${index}` });
    await vi.waitFor(() => expect(sender.failures).toBe(index + 2));
  }
  recover(); sender({ ...notice, botId: "after-outage" });
  expect(calls).toHaveLength(33);
});
