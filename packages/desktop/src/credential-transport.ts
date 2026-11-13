import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { isAbsolute, join, resolve } from "node:path";

// Windows Credential Manager for the desktop's own secrets (device keys and Logins). This is the transport the 0.4.2
// TypeScript daemon used, moved here unchanged so the desktop keeps finding what it already saved. The Rust daemon reads
// the same targets: win/cred.rs is byte-compatible with this script (generic type, local-machine persistence, user name
// `Tjommie`, UTF-8 blobs of 1 to 2560 bytes).

/**
 * The Credential Manager namespace of one data root: the first 24 hex characters of the sha256 of the lowercased, resolved
 * root (32 for the `remote-host` and `relay-login` keys, which only the Rust daemon and a test's cleanup use). A different
 * root gives different targets, which keeps dev and test installs away from production. This is the rule of
 * `Namespace::new`, `short` (24) and `long` (32) in tjommie-rs/crates/daemon/src/win/cred.rs (lines 137-159), so the
 * desktop and the Rust daemon agree on every `Tjommie/<namespace>/...` target. Changing it strands existing secrets.
 */
export const credentialNamespace = (dataRoot: string, length: 24 | 32 = 24): string =>
  createHash("sha256").update(resolve(dataRoot).toLowerCase()).digest("hex").slice(0, length);

type Operation = "read" | "write" | "delete";
export type CredentialTransport = (input: { operation: Operation; target: string; key?: string }) => Promise<string | null>;
const credentialError = () => new Error("Tjommie could not access Windows Credential Manager. The API key was not exposed or saved to a file.");

// Only the constant program appears in argv. Key material travels through anonymous stdin/stdout
// pipes. Neither a command line, environment variable, temporary file nor stderr carries the key.
const credentialScript = String.raw`
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
try {
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class TjommieCredentials {
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
 public struct Credential { public uint Flags; public uint Type; public string TargetName; public string Comment; public long LastWritten; public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist; public uint AttributeCount; public IntPtr Attributes; public string TargetAlias; public string UserName; }
 [DllImport("advapi32.dll", EntryPoint="CredWriteW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool Write(ref Credential value, uint flags);
 [DllImport("advapi32.dll", EntryPoint="CredReadW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool Read(string target, uint type, uint flags, out IntPtr value);
 [DllImport("advapi32.dll", EntryPoint="CredDeleteW", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool Delete(string target, uint type, uint flags);
 [DllImport("advapi32.dll")] static extern void CredFree(IntPtr value);
 public static string Get(string target) {
   IntPtr pointer;
   if (!Read(target, 1, 0, out pointer)) { if (Marshal.GetLastWin32Error()==1168) return null; throw new Exception(); }
   try { var value=(Credential)Marshal.PtrToStructure(pointer, typeof(Credential));
     if (value.CredentialBlobSize>2560) throw new Exception();
     byte[] bytes=new byte[value.CredentialBlobSize];
     try { Marshal.Copy(value.CredentialBlob, bytes, 0, bytes.Length); return Encoding.UTF8.GetString(bytes); }
     finally { Array.Clear(bytes,0,bytes.Length); }
   } finally { CredFree(pointer); }
 }
 public static void Put(string target,string key) {
   byte[] bytes=Encoding.UTF8.GetBytes(key); if(bytes.Length==0||bytes.Length>2560) throw new Exception();
   IntPtr pointer=Marshal.AllocHGlobal(bytes.Length);
   try { Marshal.Copy(bytes,0,pointer,bytes.Length); var value=new Credential {Type=1,TargetName=target,CredentialBlobSize=(uint)bytes.Length,CredentialBlob=pointer,Persist=2,UserName="Tjommie"}; if(!Write(ref value,0)) throw new Exception(); }
   finally { for(int i=0;i<bytes.Length;i++) Marshal.WriteByte(pointer,i,0); Marshal.FreeHGlobal(pointer); Array.Clear(bytes,0,bytes.Length); }
 }
 public static void Remove(string target) { if(!Delete(target,1,0) && Marshal.GetLastWin32Error()!=1168) throw new Exception(); }
}
'@
$payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
switch ($payload.operation) {
 'write' { [TjommieCredentials]::Put($payload.target,$payload.key); [Console]::Out.Write('null') }
 'delete' { [TjommieCredentials]::Remove($payload.target); [Console]::Out.Write('null') }
 'read' { $value=[TjommieCredentials]::Get($payload.target); if($null -eq $value) { [Console]::Out.Write('null') } else { [Console]::Out.Write((ConvertTo-Json -Compress -InputObject $value)) } }
 default { exit 1 }
}
} catch { exit 1 }
`;

export const windowsCredentialTransport: CredentialTransport = (input) => {
  if (process.platform !== "win32") return Promise.reject(credentialError());
  const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT ?? "C:\\Windows";
  if (!isAbsolute(systemRoot)) return Promise.reject(credentialError());
  const powershellRoot = join(systemRoot, "System32", "WindowsPowerShell", "v1.0");
  const env: Record<string, string> = { SystemRoot: systemRoot, WINDIR: systemRoot, PSModulePath: join(powershellRoot, "Modules") };
  for (const name of ["TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA"]) if (process.env[name]) env[name] = process.env[name];
  return new Promise((done, reject) => {
    const child = spawn(join(powershellRoot, "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", credentialScript], { env, windowsHide: true, stdio: ["pipe", "pipe", "ignore"] });
    let output = "", failed = false;
    const fail = (): void => { failed = true; reject(credentialError()); };
    const timer = setTimeout(() => { child.kill(); fail(); }, 15_000); timer.unref();
    child.once("error", () => { clearTimeout(timer); fail(); });
    child.stdin.on("error", () => { child.kill(); fail(); });
    child.stdout.on("data", (chunk: Buffer) => { if (output.length + chunk.length > 16_384) { child.kill(); fail(); } else output += chunk.toString("utf8"); });
    child.once("close", (code) => {
      clearTimeout(timer); if (failed) return;
      try { const value: unknown = JSON.parse(output); if (code !== 0 || value !== null && typeof value !== "string") throw credentialError(); done(value); }
      catch { fail(); }
    });
    child.stdin.end(JSON.stringify(input));
  });
};
