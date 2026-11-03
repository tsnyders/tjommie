import { describe, expect, it } from "vitest";
import { parseDiagnosticReport } from "./diagnostics.ts";

const valid = () => JSON.stringify({
  reportVersion: 1, device: { platform: "windows", appVersion: "0.3.2", fileBridge: "present" },
  host: { selection: "this-device", appVersion: null, daemonVersion: null, providers: [{ provider: "claude", version: null }, { provider: "codex", version: null }], relay: "unknown", storage: "unknown" },
  connection: { status: "ready", issue: null, route: "unknown" }, speech: { engine: "unknown", availability: "unknown", permission: "unknown", model: "unknown" },
}, null, 2);
describe("diagnostic export boundary", () => {
  it("accepts a fixed structured report and normalizes its JSON", () => {
    const report = valid();
    expect(parseDiagnosticReport(report)).toBe(report);
    expect(parseDiagnosticReport(JSON.stringify(JSON.parse(report)))).toBe(report);
  });
  it("rejects unknown fields, arbitrary text in known fields and credential-bearing values", () => {
    const report = JSON.parse(valid()) as Record<string, unknown>;
    expect(() => parseDiagnosticReport(JSON.stringify({ ...report, logs: ["SYNTHETIC_SECRET"] }))).toThrow("Invalid diagnostic report.");
    expect(() => parseDiagnosticReport(JSON.stringify({ ...report, device: { platform: "windows", appVersion: "0.3.2+SYNTHETIC_SECRET", fileBridge: "present" } }))).toThrow("Invalid diagnostic report.");
    expect(() => parseDiagnosticReport(JSON.stringify({ ...report, connection: { status: "failed", issue: "https://user:secret@example.test", route: "unknown" } }))).toThrow("Invalid diagnostic report.");
  });
  it("bounds byte length before JSON parsing, including multibyte input", () => {
    for (const report of ["x".repeat(16385), "é".repeat(10000), "{malformed", null]) expect(() => parseDiagnosticReport(report)).toThrow("Invalid diagnostic report.");
  });
});
