import { describe, expect, it } from "vitest";
import { diagnosticReport, diagnosticVersion } from "./diagnostics";

const input = { device: { appVersion: "0.3.2", platform: "windows" }, host: { appVersion: null, daemonVersion: "0.3.2", providers: [{ provider: "claude", version: "2.1.3" }], relay: "online", storage: "ok" }, speech: { engine: "whisper.cpp", available: false, permission: "denied", model: { state: "missing" } }, connection: { status: "offline", reason: "host_offline" }, paired: true, platform: "browser", attachments: true };

describe("diagnostic report privacy", () => {
  it("exports only structured versions and health categories, with distinct device/host scope", () => {
    expect(JSON.parse(diagnosticReport(input))).toMatchObject({ device: { appVersion: "0.3.2", platform: "windows" }, host: { appVersion: null, daemonVersion: "0.3.2", selection: "paired", providers: [{ provider: "claude", version: "2.1.3" }, { provider: "codex", version: null }] }, connection: { status: "offline", issue: "host_offline", route: "unknown" }, speech: { availability: "unavailable", model: "missing", permission: "denied" } });
  });
  it("cannot leak synthetic credentials, identity, paths, URLs, raw errors, transcripts or message contents", () => {
    const secret = "SYNTHETIC_DO_NOT_EXPORT_9f4c";
    const dirty = { ...input, device: { ...input.device, appVersion: `0.3.2-${secret}`, username: secret, path: `C:\\Users\\${secret}` }, host: { ...input.host, name: secret, token: secret, pairingCode: secret, privateKey: secret, relay: `https://user:${secret}@relay.invalid`, appVersion: secret, daemonVersion: secret, providers: [{ provider: "claude", version: `2.1.3 (${secret})`, account: secret }, { provider: secret, version: "1.2.3" }], logs: [secret] }, speech: { ...input.speech, problem: { code: secret, message: secret }, transcript: secret, audio: secret }, connection: { status: "failed", reason: secret }, password: secret, messages: [secret], fileContents: secret };
    const report = diagnosticReport(dirty);
    expect(report).not.toContain(secret);
    expect(report).not.toContain("C:\\"); expect(report).not.toContain("https:");
    expect(JSON.parse(report)).toMatchObject({ device: { appVersion: null }, host: { appVersion: null, daemonVersion: null, relay: "unknown" }, connection: { issue: "connection_failed" } });
  });
  it("handles absent and malformed fields without serializing untrusted objects", () => {
    expect(() => diagnosticReport({ device: null, host: ["wrong"], speech: { model: null }, connection: { status: "bogus", reason: { password: "synthetic" } }, paired: false, platform: "android", attachments: false })).not.toThrow();
    for (const value of [null, "v0.3.2", "0.3.2+secret", "0.3.2\nC:\\private", "https://example.test", "2.1.3 (CLI)"]) expect(diagnosticVersion(value)).toBeNull();
    expect(diagnosticVersion("0.4.0-rc.2")).toBe("0.4.0-rc.2");
  });
});
