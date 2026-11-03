import { vaultMethods, type VaultHostAction } from "../../protocol/src/vault.ts";
import { vaultHostResult } from "../../protocol/src/vault-host.ts";
import type { RequestContext } from "./request-context.ts";
import { RpcError, type Handlers } from "./server.ts";
import type { VaultHostRequest } from "./vault-host-broker.ts";

export function vaultHandlers(request: VaultHostRequest): Handlers {
  const lifetimes = new WeakSet<AbortSignal>();
  const send = async (action: VaultHostAction, context?: RequestContext) => {
    if (!context || context.signal.aborted || !context.connectionId) throw new RpcError("forbidden", "An active authenticated Logins connection is required.");
    const controllerId = `rpc:${context.connectionId}`;
    if (!lifetimes.has(context.signal)) {
      lifetimes.add(context.signal);
      context.signal.addEventListener("abort", () => { void request({ controllerId, action: { kind: "disconnect" } }).catch(() => {}); }, { once: true });
    }
    try {
      const value = await request({ controllerId, action }, { signal: context.signal, authorized: () => !context.signal.aborted });
      if (context.signal.aborted) throw new Error("cancelled");
      return vaultHostResult(action, value);
    } catch { throw new RpcError("bad_state", "Logins could not complete the request safely. Check the host; any private browser remains private."); }
  };
  return {
    "vault.status": async (params, context) => vaultMethods["vault.status"].result.parse(await send({ kind: "status", ...params }, context)),
    "vault.list": async (params, context) => vaultMethods["vault.list"].result.parse(await send({ kind: "list", ...params }, context)),
    "vault.unlock": async (params, context) => vaultMethods["vault.unlock"].result.parse(await send({ kind: "unlock", ...params }, context)),
    "vault.lock": async (params, context) => vaultMethods["vault.lock"].result.parse(await send({ kind: "lock", ...params }, context)),
    "vault.cancel": async (params, context) => vaultMethods["vault.cancel"].result.parse(await send({ kind: "cancel", ...params }, context)),
    "vault.entry": async (params, context) => vaultMethods["vault.entry"].result.parse(await send({ kind: "entry", ...params }, context)),
    "vault.delete": async (params, context) => vaultMethods["vault.delete"].result.parse(await send({ kind: "delete", ...params }, context)),
    "vault.fill": async (params, context) => vaultMethods["vault.fill"].result.parse(await send({ kind: "fill", ...params }, context)),
    "vault.finish": async (params, context) => vaultMethods["vault.finish"].result.parse(await send({ kind: "finish", ...params }, context)),
  };
}
