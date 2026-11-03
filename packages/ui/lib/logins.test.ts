import { afterEach, expect, it, vi } from "vitest";
import { createLoginsClient, LOGINS_PROBLEM } from "./logins";
import { initialState } from "./state";
import { update } from "./store";

const fixture = vi.hoisted(() => ({ call: vi.fn(), current: undefined as { call: ReturnType<typeof vi.fn> } | undefined }));
vi.mock("./connection", () => ({ daemon: () => { if (!fixture.current) throw new Error("offline"); return fixture.current; } }));
const status = { state: "unlocked", reason: null, pendingDeletes: 0 };
afterEach(() => { update(() => initialState); fixture.call.mockReset(); fixture.current = undefined; vi.unstubAllGlobals(); });
function remote() { update(() => ({ ...initialState, host: "selected-host", connection: { status: "ready" } })); fixture.current = { call: fixture.call }; }
it("captures the selected host and refuses a late response after host/client replacement", async () => {
  remote(); let finish!: (value: unknown) => void; fixture.call.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const client = createLoginsClient(), failed = expect(client.send({ kind: "status" })).rejects.toMatchObject({ name: "AbortError" });
  update(state => ({ ...state, host: "different-host" })); finish(status); await failed;
  await expect(client.send({ kind: "list" })).rejects.toThrow(LOGINS_PROBLEM); expect(fixture.call).toHaveBeenCalledOnce();
});
it("cancels the captured operation without tombstoning the connection and accepts a later fresh request", async () => {
  remote(); let finish!: (value: unknown) => void;
  fixture.call.mockImplementation(async (method: string) => method === "vault.entry" ? new Promise(resolve => { finish = resolve; }) : method === "vault.cancel" ? {} : status);
  const client = createLoginsClient(), failed = expect(client.send({ kind: "entry", botId: "one" })).rejects.toMatchObject({ name: "AbortError" });
  await client.cancel(); expect(fixture.call).toHaveBeenCalledWith("vault.cancel", {}); finish({ status: "cancelled", entry: null }); await failed;
  expect(await client.send({ kind: "status" })).toEqual(status);
});
it("uses the direct native API only for this PC, and rejects secret-bearing responses without repeating them", async () => {
  const command = vi.fn(async () => status); vi.stubGlobal("window", { tjommie: { vault: { command } } });
  expect(await createLoginsClient().send({ kind: "status" })).toEqual(status); expect(command).toHaveBeenCalledWith({ kind: "status" });
  remote(); fixture.call.mockResolvedValue({ ...status, password: "SYNTHETIC_NEVER_DISPLAY" });
  const error = await createLoginsClient().send({ kind: "status" }).catch((failure: unknown) => failure);
  expect(String(error)).toContain(LOGINS_PROBLEM); expect(String(error)).not.toContain("SYNTHETIC_NEVER_DISPLAY"); expect(command).toHaveBeenCalledOnce();
});
