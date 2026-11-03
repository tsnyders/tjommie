import { DiagnosticVersion, parseDiagnosticReport } from "@tjommie/protocol";

const object = (value: unknown): Record<string, unknown> => value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const oneOf = <T extends string>(value: unknown, choices: readonly T[], fallback: T): T => choices.find((choice) => choice === value) ?? fallback;

// Versions are data too: never copy arbitrary CLI output, build labels or error text into a report.
export function diagnosticVersion(value: unknown): string | null {
  const checked = DiagnosticVersion.safeParse(value);
  return checked.success ? checked.data : null;
}

export function diagnosticReport(input: { device: unknown; host: unknown; speech: unknown; connection: unknown; paired: boolean; platform: string; attachments: boolean }): string {
  const device = object(input.device), host = object(input.host), speech = object(input.speech), connection = object(input.connection), model = object(speech.model);
  const providers = Array.isArray(host.providers) ? host.providers : [];
  const status = oneOf(connection.status, ["connecting", "ready", "reconnecting", "offline", "removed", "update", "failed", "outside", "unknown"] as const, "unknown");
  const issue = status === "offline" ? oneOf(connection.reason, ["host_offline", "unreachable", "unknown"] as const, "unknown") : status === "removed" ? "device_removed" : status === "update" ? "version_mismatch" : status === "failed" ? "connection_failed" : null;
  return parseDiagnosticReport(JSON.stringify({
    reportVersion: 1,
    device: {
      platform: oneOf(device.platform ?? input.platform, ["windows", "macos", "linux", "android", "browser", "unknown"] as const, "unknown"),
      appVersion: diagnosticVersion(device.appVersion),
      fileBridge: input.attachments ? "present" : "unavailable",
    },
    host: {
      selection: input.paired ? "paired" : "this-device",
      appVersion: diagnosticVersion(host.appVersion), daemonVersion: diagnosticVersion(host.daemonVersion),
      providers: ["claude", "codex"].map((provider) => ({ provider, version: diagnosticVersion(object(providers.find((entry) => object(entry).provider === provider)).version) })),
      relay: oneOf(host.relay, ["off", "connecting", "online", "error", "unknown"] as const, "unknown"),
      storage: oneOf(host.storage, ["ok", "unavailable", "error", "unknown"] as const, "unknown"),
    },
    connection: { status, issue, route: "unknown" },
    speech: {
      engine: oneOf(speech.engine, ["whisper.cpp", "android-on-device", "unknown"] as const, "unknown"),
      availability: typeof speech.available === "boolean" ? speech.available ? "available" : "unavailable" : "unknown",
      permission: oneOf(speech.permission, ["unknown", "prompt", "granted", "denied"] as const, "unknown"),
      model: oneOf(model.state, ["missing", "downloading", "verifying", "ready", "error", "system-managed", "unknown"] as const, "unknown"),
    },
  }));
}
