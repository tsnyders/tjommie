import { spawnSync } from "node:child_process";
import { join } from "node:path";

// Test cleanup only. A disposable data root that a daemon protected for Codex carries deny entries for the local
// CodexSandboxUsers group on its data folders, and Windows will not delete the folders until they are gone. This is the
// `removeDeny` pass of 0.4.2's windows-acl.ts (the Rust daemon applies the same entries), moved here so the desktop tests
// need nothing from the TypeScript daemon. It changes only the paths it is given; callers pass disposable test folders.
const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT ?? "C:\\Windows";
const powershell = join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
const icacls = join(systemRoot, "System32", "icacls.exe");

const script = String.raw`
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
try {
  $sid = ([System.Security.Principal.NTAccount]::new($env:COMPUTERNAME, 'CodexSandboxUsers')).Translate([System.Security.Principal.SecurityIdentifier]).Value
} catch [System.Security.Principal.IdentityNotMappedException] {
  @{ groupMissing = $true; failures = @() } | ConvertTo-Json -Compress -Depth 4
  exit 0
}
if ($sid -notmatch '^S-1-\d+(?:-\d+)+$') { throw 'Invalid Codex sandbox group SID' }
function Test-NoDeny($path) {
  foreach ($rule in (Get-Acl -LiteralPath $path).Access) {
    try { $identity = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { continue }
    if ($identity -eq $sid -and $rule.AccessControlType -eq 'Deny') { return $false }
  }
  return $true
}
$failures = [System.Collections.Generic.List[string]]::new()
foreach ($path in $payload.paths) {
  try {
    if (-not (Test-NoDeny $path)) {
      & $payload.icacls $path '/remove:d' "*$sid" | Out-Null
      if ($LASTEXITCODE -ne 0) { throw "icacls exited with code $LASTEXITCODE" }
      if (-not (Test-NoDeny $path)) { throw 'The deny entry was not removed' }
    }
  } catch { $failures.Add("$($path): $($_.Exception.Message)") }
}
@{ groupMissing = $false; failures = @($failures) } | ConvertTo-Json -Compress -Depth 4
`;

// A daemon launched from PowerShell 7 can inherit incompatible Core modules; this pass needs only Windows PowerShell's own.
const environment = (): NodeJS.ProcessEnv => ({
  ...Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PSMODULEPATH")),
  PSModulePath: join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "Modules"),
});

export type AclCleanup = { groupMissing: boolean; failures: string[] };

export function removeCodexDenyEntries(paths: string[]): AclCleanup {
  const result = spawnSync(powershell, ["-NoProfile", "-NonInteractive", "-Command", script], {
    input: JSON.stringify({ icacls, paths }), encoding: "utf8", windowsHide: true, timeout: 60_000, maxBuffer: 1024 * 1024, env: environment(),
  });
  if (result.error) throw new Error(`The ACL cleanup could not run: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`The ACL cleanup exited with code ${result.status}: ${result.stderr.trim()}`);
  const parsed: unknown = JSON.parse(result.stdout.trim());
  if (!parsed || typeof parsed !== "object" || !("groupMissing" in parsed) || typeof parsed.groupMissing !== "boolean"
    || !("failures" in parsed) || !Array.isArray(parsed.failures) || !parsed.failures.every((failure): failure is string => typeof failure === "string")) {
    throw new Error("The ACL cleanup returned an unexpected result.");
  }
  return { groupMissing: parsed.groupMissing, failures: parsed.failures };
}
