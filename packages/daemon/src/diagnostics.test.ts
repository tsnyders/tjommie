import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { describe, expect, it } from "vitest";
import { getHostDiagnostics, readDiagnosticVersions } from "./diagnostics.ts";
import { tempRoot } from "./testing.ts";

describe("read-only host diagnostics", () => {
  it("uses cached provider versions and keeps desktop and daemon versions separate", () => {
    const db = new DatabaseSync(":memory:");
    try {
      db.exec("CREATE TABLE fixture (body TEXT); INSERT INTO fixture VALUES ('synthetic private message')");
      const setup = [{ provider: "claude" as const, version: "2.1.3", account: "SYNTHETIC_SECRET" }, { provider: "codex" as const, version: "0.155.1" }];
      const result = getHostDiagnostics({ db, setup, relay: "online", appVersion: null, daemonVersion: "0.3.2" });
      expect(result).toEqual({ appVersion: null, daemonVersion: "0.3.2", providers: [{ provider: "claude", version: "2.1.3" }, { provider: "codex", version: "0.155.1" }], relay: "online", storage: "ok" });
      expect(db.prepare("SELECT body FROM fixture").get()?.body).toBe("synthetic private message");
      expect(JSON.stringify(result)).not.toContain("SYNTHETIC_SECRET");
    } finally { db.close(); }
  });
  it("never exposes storage errors or untrusted version strings", () => {
    const result = getHostDiagnostics({ db: { prepare: () => ({ get: () => { throw new Error("SYNTHETIC_SECRET C:\\private"); } }) }, setup: [{ provider: "claude", version: "2.1.3-SYNTHETIC_SECRET" }], relay: "error", appVersion: "C:\\private", daemonVersion: "https://user:secret@example.test" });
    expect(result.storage).toBe("error");
    expect(result.providers[0]?.version).toBeNull();
    expect(result.appVersion).toBeNull(); expect(result.daemonVersion).toBeNull();
    expect(JSON.stringify(result)).not.toContain("SYNTHETIC_SECRET");
    expect(getHostDiagnostics({ db: null, setup: [], relay: "off", appVersion: null, daemonVersion: null }).storage).toBe("unavailable");
  });
  it("reads packaged version metadata, source daemon metadata, and unknowns honestly", () => {
    const root = tempRoot(), source = join(root, "src"); mkdirSync(source);
    writeFileSync(join(root, "package.json"), JSON.stringify({ name: "@tjommie/daemon", version: "0.3.2" }));
    expect(readDiagnosticVersions(source)).toEqual({ appVersion: null, daemonVersion: "0.3.2" });
    writeFileSync(join(source, "build-info.json"), JSON.stringify({ appVersion: "0.4.0", daemonVersion: "0.3.2" }));
    expect(readDiagnosticVersions(source)).toEqual({ appVersion: "0.4.0", daemonVersion: "0.3.2" });
    writeFileSync(join(source, "build-info.json"), '{"appVersion":"SYNTHETIC_SECRET","daemonVersion":"bad"}');
    expect(readDiagnosticVersions(source)).toEqual({ appVersion: null, daemonVersion: null });
    writeFileSync(join(source, "build-info.json"), "x".repeat(4097));
    expect(readDiagnosticVersions(source)).toEqual({ appVersion: null, daemonVersion: null });
    expect(readDiagnosticVersions(tempRoot())).toEqual({ appVersion: null, daemonVersion: null });
  });
});
