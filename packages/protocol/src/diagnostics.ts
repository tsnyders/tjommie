import { z } from "zod";

export const DiagnosticVersion = z.string().regex(/^\d{1,4}\.\d{1,4}\.\d{1,4}(?:-(?:alpha|beta|rc)(?:\.\d{1,4})?)?$/).nullable();
export const HostDiagnostics = z.strictObject({
  appVersion: DiagnosticVersion,
  daemonVersion: DiagnosticVersion,
  providers: z.array(z.strictObject({ provider: z.enum(["claude", "codex"]), version: DiagnosticVersion })).max(2),
  relay: z.enum(["off", "connecting", "online", "error"]),
  storage: z.enum(["ok", "unavailable", "error"]),
});
export type HostDiagnostics = z.infer<typeof HostDiagnostics>;

export const DiagnosticReport = z.strictObject({
  reportVersion: z.literal(1),
  device: z.strictObject({ platform: z.enum(["windows", "macos", "linux", "android", "browser", "unknown"]), appVersion: DiagnosticVersion, fileBridge: z.enum(["present", "unavailable"]) }),
  host: z.strictObject({
    selection: z.enum(["paired", "this-device"]), appVersion: DiagnosticVersion, daemonVersion: DiagnosticVersion,
    providers: z.array(z.strictObject({ provider: z.enum(["claude", "codex"]), version: DiagnosticVersion })).max(2),
    relay: z.enum(["off", "connecting", "online", "error", "unknown"]), storage: z.enum(["ok", "unavailable", "error", "unknown"]),
  }),
  connection: z.strictObject({
    status: z.enum(["connecting", "ready", "reconnecting", "offline", "removed", "update", "failed", "outside", "unknown"]),
    issue: z.enum(["host_offline", "unreachable", "unknown", "device_removed", "version_mismatch", "connection_failed"]).nullable(), route: z.literal("unknown"),
  }),
  speech: z.strictObject({
    engine: z.enum(["whisper.cpp", "android-on-device", "unknown"]), availability: z.enum(["available", "unavailable", "unknown"]),
    permission: z.enum(["unknown", "prompt", "granted", "denied"]), model: z.enum(["missing", "downloading", "verifying", "ready", "error", "system-managed", "unknown"]),
  }),
});

export function parseDiagnosticReport(value: unknown): string {
  if (typeof value !== "string" || value.length > 16384 || new TextEncoder().encode(value).length > 16384) throw new Error("Invalid diagnostic report.");
  let parsed: unknown;
  try { parsed = JSON.parse(value); } catch { throw new Error("Invalid diagnostic report."); }
  const result = DiagnosticReport.safeParse(parsed);
  if (!result.success) throw new Error("Invalid diagnostic report.");
  return JSON.stringify(result.data, null, 2);
}

export const diagnosticMethods = {
  "diagnostics.get": { params: z.strictObject({}), result: HostDiagnostics },
} as const;
