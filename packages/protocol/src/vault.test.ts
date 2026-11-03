import { expect, it } from "vitest";
import { LoginOrigin, VaultHostCommand, vaultMethods } from "./vault.ts";

it("accepts only canonical exact HTTPS origins", () => {
  for (const origin of ["https://example.com", "https://example.com:8443"]) expect(LoginOrigin.safeParse(origin).success).toBe(true);
  for (const origin of ["http://example.com", "https://example.com/", "https://example.com/login", "https://EXAMPLE.com", "https://u:p@example.com", "https://example.com?next=evil", "https://example.com#secret", "file:///tmp/a"]) expect(LoginOrigin.safeParse(origin).success).toBe(false);
});

it("rejects secrets and caller-supplied field selectors at every vault boundary", () => {
  const target = { id: "00000000-0000-4000-8000-000000000001", revision: 1, botId: "bot", generation: "g", pageId: "p", controlEpoch: 3 };
  expect(vaultMethods["vault.fill"].params.safeParse(target).success).toBe(true);
  for (const extra of [{ password: "synthetic" }, { username: "synthetic" }, { selector: "#other" }, { script: "return x" }, { origin: "https://evil.invalid" }]) {
    expect(vaultMethods["vault.fill"].params.safeParse({ ...target, ...extra }).success).toBe(false);
    expect(VaultHostCommand.safeParse({ controllerId: "rpc:1", action: { kind: "fill", ...target, ...extra } }).success).toBe(false);
  }
  expect(vaultMethods["vault.entry"].params.safeParse({ botId: "bot", password: "synthetic" }).success).toBe(false);
});
