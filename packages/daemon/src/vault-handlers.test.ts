import { expect, it, vi } from "vitest";
import { vaultMethods, type VaultHostCommand } from "../../protocol/src/vault.ts";
import type { RequestContext } from "./request-context.ts";
import { vaultHandlers } from "./vault-handlers.ts";

const locked = { state: "locked", reason: null, pendingDeletes: 0 };
const context = (controller = new AbortController(), local = false): RequestContext => ({ connectionId: "authenticated-one", principal: local ? { kind: "local" } : { kind: "device", deviceId: "device-one" }, signal: controller.signal });
it("derives local/device controller identity from trusted context and forwards no secret or bot payload", async () => {
  const request = vi.fn(async () => locked), handlers = vaultHandlers(request);
  await handlers["vault.status"]!({}, context());
  expect(request).toHaveBeenLastCalledWith({ controllerId: "rpc:authenticated-one", action: { kind: "status" } }, { signal: expect.any(AbortSignal), authorized: expect.any(Function) });
  await handlers["vault.lock"]!({}, context(undefined, true)); expect(request).toHaveBeenLastCalledWith({ controllerId: "rpc:authenticated-one", action: { kind: "lock" } }, expect.any(Object));
  await expect(handlers["vault.unlock"]!({})).rejects.toMatchObject({ code: "forbidden" });
  expect(vaultMethods["vault.entry"].params.safeParse({ botId: "one", password: "SYNTHETIC_NEVER_TRANSPORT" }).success).toBe(false);
});
it("sends urgent disconnect when the authenticated lifetime ends, discarding late native results", async () => {
  const abort = new AbortController(); let finish!: (value: unknown) => void;
  const request = vi.fn(async ({ action }: VaultHostCommand) => action.kind === "entry" ? new Promise<unknown>(resolve => { finish = resolve; }) : {}), handlers = vaultHandlers(request);
  const pending = handlers["vault.entry"]!({ botId: "one" }, context(abort)), rejected = expect(pending).rejects.toThrow("safely"); abort.abort();
  expect(request).toHaveBeenLastCalledWith({ controllerId: "rpc:authenticated-one", action: { kind: "disconnect" } }); finish({ status: "saved", entry: null }); await rejected;
});
it("does not expose a native error or a malformed secret-bearing result", async () => {
  const handlers = vaultHandlers(async () => { throw new Error("SYNTHETIC_SECRET_ERROR"); });
  await expect(handlers["vault.status"]!({}, context())).rejects.toThrow("Logins could not complete");
  const malformed = vaultHandlers(async () => ({ ...locked, password: "SYNTHETIC_SECRET_RESULT" }));
  await expect(malformed["vault.status"]!({}, context())).rejects.toThrow("Logins could not complete");
});
