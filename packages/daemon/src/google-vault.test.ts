import { spawn } from "node:child_process";
import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import { EventEmitter } from "node:events";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WindowsGoogleVault, windowsGoogleCipher, type GoogleCipher, type GoogleVaultRecord } from "./google-vault.ts";

vi.mock("node:child_process", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:child_process")>();
  return { ...actual, spawn: vi.fn(actual.spawn) };
});
const roots: string[] = [];
beforeEach(() => { vi.mocked(spawn).mockClear(); });
afterEach(async () => {
  vi.unstubAllEnvs();
  const actual = await vi.importActual<typeof import("node:child_process")>("node:child_process");
  vi.mocked(spawn).mockImplementation(actual.spawn);
  for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true });
});
async function directory(): Promise<string> { const root = await mkdtemp(join(tmpdir(), "tjommie-google-vault-")); roots.push(root); return root; }
const record: GoogleVaultRecord = { version: 1, client: { clientId: "123-fixture.apps.googleusercontent.com", clientSecret: "synthetic-secret" },
  connection: { refreshToken: "synthetic-refresh", subject: "fixture-user", email: "fixture@example.test", scopes: ["openid", "email", "https://www.googleapis.com/auth/gmail.readonly"], services: ["gmail"] }, grantedBotIds: ["one"] };

function fixtureCipher(): GoogleCipher {
  const key = randomBytes(32);
  return {
    protect: async (plain) => {
      const iv = randomBytes(12), cipher = createCipheriv("aes-256-gcm", key, iv);
      const content = Buffer.concat([cipher.update(plain), cipher.final()]);
      return Buffer.concat([iv, cipher.getAuthTag(), content]);
    },
    unprotect: async (encrypted) => {
      const cipher = createDecipheriv("aes-256-gcm", key, encrypted.subarray(0, 12), { authTagLength: 16 }); cipher.setAuthTag(encrypted.subarray(12, 28));
      return Buffer.concat([cipher.update(encrypted.subarray(28)), cipher.final()]);
    },
  };
}

describe("encrypted Google vault", () => {
  it("stores an atomic encrypted record below the supplied protected data directory", async () => {
    const root = await directory(), cipher = fixtureCipher(), vault = new WindowsGoogleVault(root, cipher);
    expect(await vault.load()).toBeNull(); await vault.save(record);
    const bytes = await readFile(join(root, "google", "vault.dpapi"));
    for (const secret of [record.client.clientId, record.client.clientSecret, record.connection!.refreshToken]) expect(bytes.includes(Buffer.from(secret))).toBe(false);
    expect(await new WindowsGoogleVault(root, cipher).load()).toEqual(record);
    await vault.save({ ...record, grantedBotIds: [] }); expect((await vault.load())?.grantedBotIds).toEqual([]);
    expect(await readdir(join(root, "google"))).toEqual(["vault.dpapi"]);
    await vault.clear(); expect(await vault.load()).toBeNull();
  });
  it("does not overwrite corrupt encrypted data and hides decryption details", async () => {
    const root = await directory(), vault = new WindowsGoogleVault(root, fixtureCipher());
    await mkdir(join(root, "google")); await writeFile(join(root, "google", "vault.dpapi"), "synthetic-corruption");
    await expect(vault.load()).rejects.toThrow("Google's encrypted settings could not be accessed.");
    expect(await readFile(join(root, "google", "vault.dpapi"), "utf8")).toBe("synthetic-corruption");
    const fails: GoogleCipher = { protect: async () => { throw new Error("secret-in-crypto-error"); }, unprotect: async () => { throw new Error("secret-in-crypto-error"); } };
    await expect(new WindowsGoogleVault(root, fails).load()).rejects.not.toThrow("secret-in-crypto-error");
    await expect(new WindowsGoogleVault(root, fails).save(record)).rejects.not.toThrow("secret-in-crypto-error");
    expect(await readdir(join(root, "google"))).toEqual(["vault.dpapi"]);
  });
  it("rejects an oversized or malformed stored record without disclosing its payload", async () => {
    const root = await directory(), cipher = fixtureCipher(), vault = new WindowsGoogleVault(root, cipher);
    await mkdir(join(root, "google"));
    await writeFile(join(root, "google", "vault.dpapi"), Buffer.alloc(256 * 1024 + 1));
    await expect(vault.load()).rejects.toThrow(/encrypted settings/);
    await vault.clear(); expect(await vault.load()).toBeNull();
    await writeFile(join(root, "google", "vault.dpapi"), await cipher.protect(Buffer.from(JSON.stringify({ secret: "synthetic", version: 2 }))));
    await expect(vault.load()).rejects.toThrow(/encrypted settings/);
    expect(() => new WindowsGoogleVault("relative", cipher)).toThrow(/encrypted settings/);
  });
  it.runIf(process.platform === "win32")("runs DPAPI hidden with a fixed absolute executable and stdin-only secret payload", async () => {
    const child = Object.assign(new EventEmitter(), { stdin: new PassThrough(), stdout: new PassThrough(), kill: vi.fn() });
    let input = "";
    child.stdin.on("data", (chunk: Buffer) => { input += chunk.toString(); });
    child.stdin.on("finish", () => { child.stdout.write(Buffer.from("encrypted-fixture").toString("base64")); child.emit("close", 0); });
    vi.mocked(spawn).mockReturnValue(child as unknown as ReturnType<typeof spawn>);
    vi.stubEnv("PSModulePath", "C:\\incompatible-PS7-modules"); vi.stubEnv("SYNTHETIC_TOKEN", "sensitive-env-value");
    expect((await windowsGoogleCipher.protect(Buffer.from("synthetic-secret"))).toString()).toBe("encrypted-fixture");
    const calls = vi.mocked(spawn).mock.calls as unknown as [string, string[], { windowsHide: boolean; env: NodeJS.ProcessEnv; stdio: string[] }][];
    const [file, args, options] = calls[0];
    expect(file).toMatch(/^[A-Z]:\\.*\\System32\\WindowsPowerShell\\v1\.0\\powershell\.exe$/i);
    expect(args).toContain("-NoProfile"); expect(args).toContain("-NonInteractive"); expect(options.windowsHide).toBe(true);
    expect(options.env.PSModulePath).toMatch(/WindowsPowerShell\\v1\.0\\Modules$/);
    expect(options.env.SYNTHETIC_TOKEN).toBeUndefined(); expect(options.stdio).toEqual(["pipe", "pipe", "ignore"]);
    expect(JSON.stringify([args, options])).not.toContain("synthetic-secret");
    const payload: unknown = JSON.parse(input);
    expect(payload).toEqual({ mode: "protect", data: Buffer.from("synthetic-secret").toString("base64") });
    expect(args.join(" ")).toContain("DataProtectionScope]::CurrentUser");
  });
  it.runIf(process.platform === "win32")("round-trips disposable synthetic data through real Windows DPAPI", async () => {
    const root = await directory(), vault = new WindowsGoogleVault(root);
    await vault.save(record);
    expect((await readFile(join(root, "google", "vault.dpapi"))).includes(Buffer.from("synthetic-refresh"))).toBe(false);
    expect(await new WindowsGoogleVault(root).load()).toEqual(record);
  });
});
