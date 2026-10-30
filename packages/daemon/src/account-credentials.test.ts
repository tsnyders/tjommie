import { randomUUID } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { WindowsAccountCredentials, validateApiKey, type CredentialTransport } from "./account-credentials.ts";
import { tempRoot } from "./testing.ts";

describe("account credentials", () => {
  it("isolates opaque references by installation and account without exposing keys", async () => {
    const transport = vi.fn<CredentialTransport>().mockResolvedValue(null);
    const first = new WindowsAccountCredentials("C:/Tjommie/one", transport);
    const second = new WindowsAccountCredentials("C:/Tjommie/two", transport);
    expect(first.reference("account-a")).not.toBe(first.reference("account-b"));
    expect(first.reference("account-a")).not.toBe(second.reference("account-a"));
    expect(first.reference("account-a")).not.toContain("account-a");
    await first.write("account-a", "synthetic-key");
    expect(transport).toHaveBeenCalledExactlyOnceWith({ operation: "write", target: first.reference("account-a"), key: "synthetic-key" });
    expect(await first.read("account-a")).toBeNull();
    await first.delete("account-a");
    expect(transport).toHaveBeenLastCalledWith({ operation: "delete", target: first.reference("account-a") });
  });
  it("enforces the native UTF-8 blob bound before accessing the credential store", async () => {
    const transport = vi.fn<CredentialTransport>();
    const credentials = new WindowsAccountCredentials(tempRoot(), transport);
    for (const value of ["", " secret", "a\nb", "a".repeat(2561), "é".repeat(1281)]) await expect(credentials.write("a", value)).rejects.toThrow("2560");
    expect(transport).not.toHaveBeenCalled();
    expect(() => validateApiKey("é".repeat(1280))).not.toThrow();
  });
  it("redacts backend failures and rejects corrupt stored values", async () => {
    const credentials = new WindowsAccountCredentials(tempRoot(), async () => { throw new Error("synthetic-secret payload"); });
    await expect(credentials.read("a")).rejects.toThrow("Windows Credential Manager");
    await expect(credentials.write("a", "synthetic-secret")).rejects.not.toThrow("synthetic-secret");
    const invalid = new WindowsAccountCredentials(tempRoot(), async () => "has a space");
    await expect(invalid.read("a")).rejects.toThrow("Windows Credential Manager");
  });
  // Hosted Windows service logons cannot reliably reach Credential Manager; local Windows still runs this round trip.
  it.skipIf(process.platform !== "win32" || process.env.TJOMMIE_CI_HOSTED === "1")("round trips only a fresh synthetic Windows credential and deletes it", async () => {
    const credentials = new WindowsAccountCredentials(tempRoot());
    const id = `synthetic-test-${randomUUID()}`;
    const key = `synthetic-${randomUUID()}-é`;
    try {
      expect(await credentials.read(id)).toBeNull();
      await credentials.write(id, key);
      expect(await credentials.read(id)).toBe(key);
    } finally { await credentials.delete(id); }
    expect(await credentials.read(id)).toBeNull();
  }, 45_000);
});
