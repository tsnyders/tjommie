import type { Client } from "@tjommie/client";
import { VaultHostAction, type VaultHostAction as Action } from "../../protocol/src/vault";
import { vaultHostResult } from "../../protocol/src/vault-host";
import { daemon } from "./connection";
import { desktop } from "./desktop";
import { getState } from "./store";

export const LOGINS_PROBLEM = "Logins could not complete the request safely. Check the selected host. Any private browser remains private until its fields are cleared or it is closed.";
const cancelled = () => new DOMException("Logins request cancelled.", "AbortError");
/** Capture one selected host/transport so a late operation can never target a newly selected PC. */
export function createLoginsClient() {
  const host = getState().host, native = host === null ? desktop()?.vault : undefined;
  let client: Pick<Client, "call"> | undefined;
  if (!native) try { client = daemon(); } catch { /* Honest unavailable until this host connects. */ }
  let epoch = 0, pending = 0;
  const current = () => {
    if (getState().host !== host) return false;
    if (native) return desktop()?.vault === native;
    try { return getState().connection.status === "ready" && daemon() === client; } catch { return false; }
  };
  const invoke = async (action: Action): Promise<unknown> => {
    if (native) return native.command(action);
    if (!client) throw new Error(LOGINS_PROBLEM);
    const { kind, ...params } = action;
    return kind === "disconnect" ? client.call("vault.cancel", {}) : client.call(`vault.${kind}`, params);
  };
  return {
    current,
    async send(raw: Action) {
      const action = VaultHostAction.parse(raw);
      if (!current()) throw new Error(LOGINS_PROBLEM);
      if (["lock", "cancel", "disconnect"].includes(action.kind)) epoch++;
      const revision = epoch, interactive = !["status", "list", "lock", "cancel", "disconnect"].includes(action.kind);
      if (interactive) pending++;
      try { const result = vaultHostResult(action, await invoke(action)); if (!current() || revision !== epoch) throw cancelled(); return result; }
      catch (error) { if (!current() || revision !== epoch || error instanceof DOMException && error.name === "AbortError") throw cancelled(); throw new Error(LOGINS_PROBLEM); }
      finally { if (interactive) pending--; }
    },
    async cancel(): Promise<void> {
      epoch++;
      if (pending) await invoke({ kind: "cancel" }).catch(() => {});
    },
  };
}
