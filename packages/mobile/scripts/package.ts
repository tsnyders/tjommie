import { execFileSync, execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { buildInputHash } from "../../desktop/scripts/packaged-provenance.ts";

const repo = resolve(import.meta.dirname, "../../..");
if (process.platform !== "win32") throw new Error("This packaging script currently verifies Android artifacts on Windows.");
execSync("npm run build", { cwd: repo, stdio: "inherit" });
execSync("npm run android:sync -w @tjommie/mobile", { cwd: repo, stdio: "inherit" });
const inputsSha256 = buildInputHash(repo);
execSync("npm run android:debug -w @tjommie/mobile", { cwd: repo, stdio: "inherit" });

const apk = join(repo, "packages/mobile/android/app/build/outputs/apk/debug/app-debug.apk");
const script = String.raw`
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$inputData=[Console]::In.ReadToEnd() | ConvertFrom-Json
$zip=[System.IO.Compression.ZipFile]::OpenRead($inputData.apk)
try {
 $result=@(foreach($entry in $zip.Entries) {
   if($entry.FullName.StartsWith('assets/public/') -and !$entry.FullName.EndsWith('/')) {
     $stream=$entry.Open(); $hash=[System.Security.Cryptography.SHA256]::Create()
     try { $digest=([BitConverter]::ToString($hash.ComputeHash($stream))).Replace('-','').ToLowerInvariant(); [PSCustomObject]@{path=$entry.FullName.Substring(14);sha256=$digest} }
     finally { $hash.Dispose(); $stream.Dispose() }
   }
 })
 ConvertTo-Json -Compress -Depth 3 -InputObject $result
} finally { $zip.Dispose() }
`;
const output = execFileSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32/WindowsPowerShell/v1.0/powershell.exe"), ["-NoProfile", "-NonInteractive", "-Command", script], { input: JSON.stringify({ apk }), encoding: "utf8", windowsHide: true, maxBuffer: 4 * 1024 * 1024 });
const entries: unknown = JSON.parse(output);
if (!Array.isArray(entries)) throw new Error("Could not verify the APK's bundled UI.");
const hashes = new Map<string, string>();
for (const entry of entries as unknown[]) {
  if (typeof entry !== "object" || entry === null || !("path" in entry) || !("sha256" in entry) || typeof entry.path !== "string" || typeof entry.sha256 !== "string" || hashes.has(entry.path)) throw new Error("Invalid APK asset entry.");
  hashes.set(entry.path, entry.sha256);
}
let assets = 0;
const digest = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const ui = join(repo, "packages/ui/out");
for (const file of readdirSync(ui, { recursive: true, withFileTypes: true })) {
  if (!file.isFile()) continue;
  const absolute = join(file.parentPath, file.name);
  const relative = absolute.slice(ui.length + 1).replace(/\\/g, "/");
  if (hashes.get(relative) !== digest(absolute)) throw new Error(`The APK has stale or missing UI bytes: ${relative}`);
  assets++;
}
if (assets === 0 || buildInputHash(repo) !== inputsSha256) throw new Error("Android inputs changed during packaging. Run npm run package:android again.");
const out = join(repo, "dist-mobile"); mkdirSync(out, { recursive: true });
const version = (JSON.parse(readFileSync(join(repo, "packages/mobile/package.json"), "utf8")) as { version: string }).version;
const artifact = `Tjommie Android ${version} debug.apk`;
cpSync(apk, join(out, artifact));
const provenance = { version: 1, inputsSha256, artifact, sha256: digest(apk), uiAssetsVerified: assets, signing: "Android debug signing; local testing only" };
writeFileSync(join(out, "provenance.json"), JSON.stringify(provenance, null, 2));
console.log(`Android package verified: ${assets} UI assets match the current build. ${join(out, artifact)}`);
