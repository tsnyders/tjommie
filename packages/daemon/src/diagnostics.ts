import { DiagnosticVersion, HostDiagnostics, type Provider } from "@tjommie/protocol";
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { join } from "node:path";

type StorageProbe = { prepare(sql: string): { get(): unknown } };
type DiagnosticsInput = {
  db: StorageProbe | null;
  setup: readonly { provider: Provider; version: string | null }[];
  relay: HostDiagnostics["relay"];
  appVersion: unknown;
  daemonVersion: unknown;
};
const version = (value: unknown): string | null => { const checked = DiagnosticVersion.safeParse(value); return checked.success ? checked.data : null; };

export function readDiagnosticVersions(directory = import.meta.dirname): { appVersion: string | null; daemonVersion: string | null } {
  const buildInfo = join(directory, "build-info.json");
  const packaged = existsSync(buildInfo);
  try {
    const path = packaged ? buildInfo : join(directory, "..", "package.json");
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 4096) return { appVersion: null, daemonVersion: null };
    const value: unknown = JSON.parse(readFileSync(path, "utf8"));
    if (!value || typeof value !== "object") return { appVersion: null, daemonVersion: null };
    if (packaged) return { appVersion: version("appVersion" in value ? value.appVersion : null), daemonVersion: version("daemonVersion" in value ? value.daemonVersion : null) };
    return { appVersion: null, daemonVersion: "name" in value && value.name === "@tjommie/daemon" ? version("version" in value ? value.version : null) : null };
  } catch { return { appVersion: null, daemonVersion: null }; }
}

// Cached provider metadata and one bounded read-only database check: no account access or CLI/model invocation.
export function getHostDiagnostics(input: DiagnosticsInput): HostDiagnostics {
  let storage: HostDiagnostics["storage"] = "unavailable";
  if (input.db) {
    try {
      const row = input.db.prepare("PRAGMA quick_check(1)").get();
      storage = row && typeof row === "object" && "quick_check" in row && row.quick_check === "ok" ? "ok" : "error";
    } catch { storage = "error"; }
  }
  return HostDiagnostics.parse({
    appVersion: version(input.appVersion), daemonVersion: version(input.daemonVersion),
    providers: (["claude", "codex"] as const).map((provider) => ({ provider, version: version(input.setup.find((item) => item.provider === provider)?.version) })),
    relay: input.relay, storage,
  });
}
