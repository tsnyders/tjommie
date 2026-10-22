import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { lstat, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { z } from "zod";
import { GoogleClientConfig, GoogleService } from "../../protocol/src/google.ts";

export const GoogleVaultRecord = z.object({
  version: z.literal(1), client: GoogleClientConfig,
  connection: z.object({
    refreshToken: z.string().min(1).max(16_384), subject: z.string().min(1).max(255),
    email: z.string().email().max(320).nullable(), scopes: z.array(z.string().max(512)).max(30),
    services: z.array(GoogleService).max(3),
  }).strict().nullable(),
  grantedBotIds: z.array(z.string().min(1).max(200)).max(1000),
}).strict();
export type GoogleVaultRecord = z.infer<typeof GoogleVaultRecord>;
export interface GoogleVault {
  load(): Promise<GoogleVaultRecord | null>;
  save(record: GoogleVaultRecord): Promise<void>;
  clear(): Promise<void>;
}
export interface GoogleCipher {
  protect(value: Buffer): Promise<Buffer>;
  unprotect(value: Buffer): Promise<Buffer>;
}
const LIMIT = 256 * 1024;
const vaultError = (): Error => new Error("Google's encrypted settings could not be accessed. Check this Windows account and the app's data folder.");
const missing = (error: unknown): boolean => error instanceof Error && "code" in error && error.code === "ENOENT";

// Only this constant script is passed in argv. Secrets travel through anonymous pipes; no
// PowerShell profile, credentials in arguments/environment, transcripts, or stderr are used.
const dpapiScript = `
$ErrorActionPreference = 'Stop'
try {
  Add-Type -AssemblyName System.Security
  $payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
  $bytes = [Convert]::FromBase64String($payload.data)
  $scope = [System.Security.Cryptography.DataProtectionScope]::CurrentUser
  if ($payload.mode -eq 'protect') {
    $result = [System.Security.Cryptography.ProtectedData]::Protect($bytes, $null, $scope)
  } elseif ($payload.mode -eq 'unprotect') {
    $result = [System.Security.Cryptography.ProtectedData]::Unprotect($bytes, $null, $scope)
  } else { exit 1 }
  [Console]::Out.Write([Convert]::ToBase64String($result))
} catch { exit 1 }
`;

function dpapi(mode: "protect" | "unprotect", value: Buffer): Promise<Buffer> {
  if (process.platform !== "win32" || value.length > LIMIT) return Promise.reject(vaultError());
  const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT ?? "C:\\Windows";
  if (!isAbsolute(systemRoot)) return Promise.reject(vaultError());
  const home = join(systemRoot, "System32", "WindowsPowerShell", "v1.0");
  const env: NodeJS.ProcessEnv = { SystemRoot: systemRoot, WINDIR: systemRoot, PSModulePath: join(home, "Modules") };
  for (const key of ["TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA"]) if (process.env[key]) env[key] = process.env[key];
  return new Promise((resolve, reject) => {
    const child = spawn(join(home, "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", dpapiScript], {
      windowsHide: true, env, stdio: ["pipe", "pipe", "ignore"],
    });
    let output = "", failed = false;
    const fail = (): void => { failed = true; reject(vaultError()); };
    const timer = setTimeout(() => { child.kill(); fail(); }, 15_000);
    timer.unref();
    child.once("error", () => { clearTimeout(timer); fail(); });
    child.stdin.on("error", () => { child.kill(); fail(); });
    child.stdout.on("data", (chunk: Buffer) => {
      if (output.length + chunk.length > LIMIT * 2) { child.kill(); fail(); }
      else output += chunk.toString("ascii");
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (failed) return;
      if (code !== 0 || !output || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(output)) { fail(); return; }
      const bytes = Buffer.from(output, "base64");
      if (bytes.length > LIMIT) { fail(); return; }
      resolve(bytes);
    });
    child.stdin.end(JSON.stringify({ mode, data: value.toString("base64") }));
  });
}

export const windowsGoogleCipher: GoogleCipher = {
  protect: (value) => dpapi("protect", value), unprotect: (value) => dpapi("unprotect", value),
};

export class WindowsGoogleVault implements GoogleVault {
  readonly #directory: string;
  readonly #file: string;
  readonly #cipher: GoogleCipher;
  // Parent passes Paths.data, whose inherited ACL excludes sandboxed bots. DPAPI binds
  // ciphertext to the Windows user; it is not isolation from other code run as that user.
  constructor(protectedDataDir: string, cipher: GoogleCipher = windowsGoogleCipher) {
    if (!isAbsolute(protectedDataDir)) throw vaultError();
    this.#directory = join(protectedDataDir, "google");
    this.#file = join(this.#directory, "vault.dpapi");
    this.#cipher = cipher;
  }
  async #check(checkSize = true): Promise<void> {
    await mkdir(this.#directory, { recursive: true, mode: 0o700 });
    const directory = await lstat(this.#directory);
    if (!directory.isDirectory() || directory.isSymbolicLink()) throw vaultError();
    try {
      const file = await lstat(this.#file);
      if (!file.isFile() || file.isSymbolicLink() || (checkSize && file.size > LIMIT)) throw vaultError();
    } catch (error) { if (!missing(error)) throw error; }
  }
  async load(): Promise<GoogleVaultRecord | null> {
    try {
      await this.#check();
      let bytes: Buffer;
      try { bytes = await readFile(this.#file); } catch (error) { if (missing(error)) return null; throw error; }
      const plain = await this.#cipher.unprotect(bytes);
      try { return GoogleVaultRecord.parse(JSON.parse(plain.toString("utf8"))); }
      finally { plain.fill(0); }
    } catch { throw vaultError(); }
  }
  async save(record: GoogleVaultRecord): Promise<void> {
    const temporary = join(this.#directory, `${randomUUID()}.tmp`);
    try {
      await this.#check();
      const plain = Buffer.from(JSON.stringify(GoogleVaultRecord.parse(record)), "utf8");
      let encrypted: Buffer;
      try { if (plain.length > LIMIT) throw vaultError(); encrypted = await this.#cipher.protect(plain); }
      finally { plain.fill(0); }
      if (!encrypted.length || encrypted.length > LIMIT) throw vaultError();
      await writeFile(temporary, encrypted, { flag: "wx", mode: 0o600 });
      await rename(temporary, this.#file);
    } catch { throw vaultError(); }
    finally { await rm(temporary, { force: true }).catch(() => {}); }
  }
  async clear(): Promise<void> {
    try { await this.#check(false); await rm(this.#file, { force: true }); }
    catch { throw vaultError(); }
  }
}
