import { afterEach, expect, it, vi } from "vitest";
import type { EventData, PushKind } from "@tjommie/protocol";
import { startPush } from "./push.ts";

const notice: EventData<"notify"> = { kind: "approval", botId: "bot-1", title: "private content", body: "private detail" };
const done: EventData<"notify"> = { kind: "done", botId: "bot-1", title: "private content", body: "private detail", roomId: "team" };
type Result = { ok: boolean; unregistered: boolean };
function setup(count = 1) {
  const targets: { deviceId: string; token: string; kinds: PushKind[] | null }[] = Array.from({ length: count }, (_, index) => ({ deviceId: `device-${index}`, token: `synthetic-token-${index}`, kinds: null }));
  const cleared: { deviceId: string; token: string }[] = [];
  const calls: { tokens: string[]; kind: PushKind; botId: string; roomId?: string }[] = [];
  let results: Result[] | null = null;
  let reject = false;
  let supported: PushKind[] = ["approval", "limit", "routine_failed", "done", "failed"];
  const sender = startPush({
    host: { pushTargets: () => targets, clearPushToken: (deviceId, token) => { cleared.push({ deviceId, token }); } },
    relay: { kinds: () => supported, push: async (tokens, kind, botId, roomId) => { calls.push({ tokens, kind, botId, roomId }); if (reject) throw new Error("synthetic-token-0"); return results && calls.length === 1 ? results : tokens.map(() => ({ ok: true, unregistered: false })); } },
  });
  return { sender, calls, cleared, targets, setKinds: (kinds: PushKind[]) => { supported = kinds; }, setResults: (value: Result[]) => { results = value; }, fail: () => { reject = true; }, recover: () => { reject = false; } };
}
afterEach(() => vi.useRealTimers());

it("coalesces by device, kind and bot, then enforces 30 non-approval pushes per device per hour", async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  const { sender, calls } = setup();
  sender(done); sender(done);
  await vi.waitFor(() => expect(calls).toHaveLength(1));
  vi.setSystemTime(30_000);
  for (let i = 0; i < 29; i++) sender({ ...done, botId: `bot-${i}` });
  await vi.waitFor(() => expect(calls).toHaveLength(30));
  sender({ ...done, botId: "overflow" });
  expect(calls).toHaveLength(30);
  sender(notice);
  expect(calls).toHaveLength(31);
  vi.setSystemTime(3_600_001);
  sender(done);
  await vi.waitFor(() => expect(calls).toHaveLength(32));
});

it("filters per device and relay capability and forwards only room routing metadata", async () => {
  const { sender, calls, targets, setKinds } = setup(2);
  targets[0].kinds = ["approval"];
  targets[1].kinds = ["done"];
  setKinds(["approval", "limit", "routine_failed"]);
  sender(done);
  expect(calls).toEqual([]);
  setKinds(["approval", "done"]);
  sender(done);
  await vi.waitFor(() => expect(calls).toHaveLength(1));
  expect(calls).toEqual([{ tokens: ["synthetic-token-1"], kind: "done", botId: "bot-1", roomId: "team" }]);
  expect(JSON.stringify(calls)).not.toContain("private");
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
  fail(); sender(done);
  await vi.waitFor(() => expect(sender.failures).toBe(1));
  recover(); vi.setSystemTime(10_000); sender(done);
  expect(calls).toHaveLength(2);
  for (let index = 0; index < 30; index++) {
    fail(); sender({ ...done, botId: `failed-${index}` });
    await vi.waitFor(() => expect(sender.failures).toBe(index + 2));
  }
  recover(); sender({ ...done, botId: "after-outage" });
  expect(calls).toHaveLength(33);
});
