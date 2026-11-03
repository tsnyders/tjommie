import { expect, it, vi } from "vitest";
import { VaultCommandGate } from "./vault-command-gate.ts";

function fixture() {
  let resolved: () => void = () => {};
  const lookup = new Promise<void>((done) => { resolved = done; });
  const vault = { lock: vi.fn(), command: vi.fn(async (_command: Parameters<VaultCommandGate["execute"]>[0]) => ({})) };
  let connection = {};
  const gate = new VaultCommandGate({ vault: () => vault, ready: () => true, connection: () => connection, resolveBot: () => lookup });
  const entry = { controllerId: "rpc:one", action: { kind: "entry", botId: "bot" } } as const;
  return { gate, vault, entry, resolved, reconnect: () => { connection = {}; } };
}

it("cancels a delayed bot lookup before native entry starts and allows a subsequent request", async () => {
  const h = fixture(), pending = h.gate.execute(h.entry);
  await h.gate.execute({ controllerId: "rpc:one", action: { kind: "cancel" } });
  h.resolved(); await expect(pending).rejects.toThrow("connection changed");
  expect(h.vault.command).toHaveBeenCalledTimes(1);
  await h.gate.execute(h.entry);
  expect(h.vault.command).toHaveBeenLastCalledWith(h.entry);
});

it.each(["lock", "disconnect", "navigation", "reconnect"] as const)("fences delayed native work after %s", async (kind) => {
  const h = fixture(), pending = h.gate.execute(h.entry);
  if (kind === "navigation") h.gate.invalidate();
  else if (kind === "reconnect") h.reconnect();
  else await h.gate.execute({ controllerId: kind === "lock" ? "local" : "rpc:one", action: { kind } });
  h.resolved(); await expect(pending).rejects.toThrow("connection changed");
  expect(h.vault.command.mock.calls.some(([command]) => command?.action.kind === "entry")).toBe(false);
});
