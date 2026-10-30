import { execFileSync, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { deflateRawSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import {
  assertTestDefines, assertTestDownloadedUpdate, assertTestInstaller, assertTestRegistration, assertTestUpdate, readRegistryKeys,
  testIdentity, testNsisInclude, testRegistryKeys, type RegistryKey,
} from "../scripts/update-test-guards.ts";

const defines = {
  APP_ID: "com.tsnyders.tjommie.updatetest", APP_GUID: "5891e549-73c7-5ca5-865f-6a0f99687a40",
  UNINSTALL_APP_KEY: "5891e549-73c7-5ca5-865f-6a0f99687a40", APP_FILENAME: "tjommie-updatetest",
  APP_PACKAGE_NAME: "tjommie-updatetest", APP_INSTALLER_STORE_FILE: "tjommie-updatetest-updater\\installer.exe",
  PRODUCT_NAME: "Tjommie Update Test", PRODUCT_FILENAME: "Tjommie Update Test",
  APP_PRODUCT_FILENAME: "Tjommie Update Test", SHORTCUT_NAME: "Tjommie Update Test",
};

it("rejects a shared filename or production GUID before either NSIS stage", () => {
  expect(() => assertTestDefines([defines, {}])).not.toThrow();
  for (const [key, value] of Object.entries(defines)) {
    expect(() => assertTestDefines([{ ...defines, [key]: key.includes("GUID") || key === "UNINSTALL_APP_KEY"
      ? "89b4e4b4-ac49-585f-800a-537f559f4641" : value === "tjommie-updatetest" ? "tjommie" : "wrong" }, {}])).toThrow(key);
  }
  expect(() => assertTestDefines([{ ...defines, UNINSTALL_REGISTRY_KEY_2: "Software\\production" }, {}])).toThrow();
});

// Minimal NSIS 3 Unicode/zlib header and data blocks, with the same string-table layout as the compiler.
function binary(strings: string[], nested?: Buffer): Buffer {
  const table = Buffer.from(`\0${strings.join("\0")}\0`, "utf16le");
  const header = Buffer.alloc(300);
  header.writeUInt32LE(300, 28);
  header.writeUInt32LE(300 + table.length, 36);
  const expanded = Buffer.concat([header, table]);
  const block = (input: Buffer): Buffer => {
    const compressed = deflateRawSync(input), size = Buffer.alloc(4);
    size.writeUInt32LE((compressed.length | 0x80000000) >>> 0);
    return Buffer.concat([size, compressed]);
  };
  const first = Buffer.alloc(28);
  Buffer.from("efbeadde4e756c6c736f6674496e7374", "hex").copy(first, 4);
  first.writeUInt32LE(expanded.length, 20);
  const data = Buffer.concat([block(expanded), ...(nested ? [block(nested)] : [])]);
  first.writeUInt32LE(first.length + data.length, 24);
  return Buffer.concat([first, data]);
}

function fixtureStrings(install: string): string[] {
  return [
    ...testRegistryKeys().filter((key) => key.hive === "HKCU" && key.view === 64).map((key) => key.key),
    testIdentity.appId, testIdentity.product, `\u0003\u8095\\${testIdentity.product}.exe`,
    `\u0003\u8080\\${testIdentity.packageName}`, `\u0002\u231c\\${testIdentity.packageName}-updater\\installer.exe`,
    `Uninstall ${testIdentity.product}.exe`, install, "Tjommie update-test target guard",
  ];
}

it("checks both compiled headers, including identity drift inside the embedded uninstaller", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-guard-binary-"));
  const install = join(root, testIdentity.product), file = join(root, "installer.exe");
  const strings = fixtureStrings(install);
  try {
    writeFileSync(file, binary(strings, binary(strings)));
    expect(() => assertTestInstaller(file, install)).not.toThrow();
    // NSIS may reference the product-name suffix of the already stored absolute install path.
    const interned = strings.filter((value) => value !== testIdentity.product);
    writeFileSync(file, binary(interned, binary(interned)));
    expect(() => assertTestInstaller(file, install)).not.toThrow();
    for (const production of ["89B4E4B4-AC49-585F-800A-537F559F4641", "prefix:com.tsnyders.tjommie", "\\Tjommie", "\\TJOMMIE.EXE", "\\UNINSTALL TJOMMIE.EXE", "\\TJOMMIE-UPDATER\\INSTALLER.EXE"]) {
      // Keep every required identity intact so only the deny-list can reject these.
      writeFileSync(file, binary([...strings, production], binary(strings)));
      expect(() => assertTestInstaller(file, install)).toThrow(/Production identity/);
      writeFileSync(file, binary(strings, binary([...strings, production])));
      expect(() => assertTestInstaller(file, install)).toThrow(/Production identity/);
    }
    const unguarded = strings.filter((value) => value !== "Tjommie update-test target guard");
    for (const contents of [binary(unguarded, binary(strings)), binary(strings, binary(unguarded))]) {
      writeFileSync(file, contents);
      expect(() => assertTestInstaller(file, install)).toThrow(/target guard/);
    }
    writeFileSync(file, binary(strings, binary(strings.map((s) => s.replace(testIdentity.guid, "89b4e4b4-ac49-585f-800a-537f559f4641")))));
    expect(() => assertTestInstaller(file, install)).toThrow();
    writeFileSync(file, binary(strings.filter((s) => !s.endsWith("\\tjommie-updatetest")), binary(strings)));
    expect(() => assertTestInstaller(file, install)).toThrow();
    writeFileSync(file, binary(strings, binary(strings)));
    expect(() => assertTestInstaller(file, join(root, "different"))).toThrow();
    writeFileSync(file, "not NSIS");
    expect(() => assertTestInstaller(file, install)).toThrow();
  } finally { rmSync(root, { recursive: true, force: true }); }
});

it("pins update handoff to the manifest, installer identity and downloaded sha512", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-guard-download-"));
  const install = join(root, testIdentity.product), manifestFile = join(root, "latest.yml"), pending = join(root, "pending");
  const version = "0.3.1-test", name = `Tjommie-Setup-${version}.exe`, file = join(root, name);
  const strings = fixtureStrings(install), bytes = binary(strings, binary(strings));
  const sha512 = createHash("sha512").update(bytes).digest("base64");
  const manifest = { version, path: name, sha512, files: [{ url: name, sha512, size: bytes.length }] };
  const saveManifest = (value: unknown = manifest): void => writeFileSync(manifestFile, JSON.stringify(value));
  try {
    writeFileSync(file, bytes);
    writeFileSync(manifestFile, `version: ${version}\npath: ${name}\nsha512: ${sha512}\nfiles:\n  - url: ${name}\n    sha512: ${sha512}\n    size: ${bytes.length}\n`);
    expect(() => assertTestUpdate(manifestFile, file, install, version)).not.toThrow();
    for (const bad of [null, {}, { ...manifest, version: "0.1.0" }, { ...manifest, path: "../production.exe" },
      { ...manifest, sha512: "wrong" }, { ...manifest, files: [] }, { ...manifest, files: [...manifest.files, ...manifest.files] },
      { ...manifest, files: [{ ...manifest.files[0], url: "https://example.invalid/owner.exe" }] },
      { ...manifest, files: [{ ...manifest.files[0], sha512: "wrong" }] },
      { ...manifest, files: [{ ...manifest.files[0], size: bytes.length + 1 }] },
      { ...manifest, packages: { x64: { path: "unverified.7z" } } }]) {
      saveManifest(bad);
      expect(() => assertTestUpdate(manifestFile, file, install, version)).toThrow();
    }
    saveManifest();
    mkdirSync(pending);
    const downloaded = join(pending, name), cacheInfo = join(pending, "update-info.json");
    writeFileSync(downloaded, bytes);
    writeFileSync(cacheInfo, JSON.stringify({ fileName: name, sha512, isAdminRightsRequired: false }));
    expect(() => assertTestDownloadedUpdate(manifestFile, pending, install, version)).not.toThrow();
    writeFileSync(downloaded, Buffer.concat([bytes, Buffer.from("changed after download")]));
    expect(() => assertTestDownloadedUpdate(manifestFile, pending, install, version)).toThrow(/manifest|sha512/i);
    writeFileSync(downloaded, bytes);
    for (const bad of [{ fileName: "../owner.exe", sha512 }, { fileName: name, sha512: "wrong" }, { fileName: name, sha512, isAdminRightsRequired: true }]) {
      writeFileSync(cacheInfo, JSON.stringify(bad));
      expect(() => assertTestDownloadedUpdate(manifestFile, pending, install, version)).toThrow(/download/i);
    }
    // A matching checksum cannot bless an installer with the wrong embedded identity.
    const unsafe = binary([...strings, "\\Tjommie"], binary(strings));
    const unsafeHash = createHash("sha512").update(unsafe).digest("base64");
    writeFileSync(downloaded, unsafe);
    writeFileSync(cacheInfo, JSON.stringify({ fileName: name, sha512: unsafeHash }));
    saveManifest({ ...manifest, sha512: unsafeHash, files: [{ url: name, sha512: unsafeHash, size: unsafe.length }] });
    expect(() => assertTestDownloadedUpdate(manifestFile, pending, install, version)).toThrow(/Production identity/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

it("generates both target invariants and old-version registry validation only for the test include", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-guard-include-"));
  try {
    const include = testNsisInclude(join(root, testIdentity.product));
    expect(include).toContain("!macro customInit");
    expect(include).toContain("!macro customUnInit");
    expect(include).toContain("GetFullPathName");
    expect(include).toContain("SetErrorLevel 91");
    expect(include).toContain("SetRegView 32");
    expect(include).toContain("SetRegView 64");
    expect(include).toContain("HKLM");
    expect(include).not.toContain("89b4e4b4-ac49-585f-800a-537f559f4641");
    expect(include).not.toContain("TEST_INSTALL_DIR");
    expect(include).not.toContain("GetOptions");
    expect(include).not.toContain("StrCpy $INSTDIR");
    expect(() => testNsisInclude(join(root, "wrong"))).toThrow();
  } finally { rmSync(root, { recursive: true, force: true }); }
});

describe.skipIf(process.platform !== "win32")("disposable HKCU registration fixtures", () => {
  it.each([false, true])("aborts before removal and preserves an early failure (fail before key creation: %s)", (failEarly) => {
    const root = mkdtempSync(join(tmpdir(), "tjommie-guard-registry-"));
    const install = join(root, testIdentity.product), sentinel = join(root, "must-survive");
    const key = `Software\\TjommieGuardFixtures\\${randomUUID()}`;
    const keys: RegistryKey[] = [{ hive: "HKCU", view: 64, key: `${key}\\Install` }, { hive: "HKCU", view: 64, key: `${key}\\Uninstall` }];
    const reg = (args: string[]): void => { execFileSync("reg.exe", args, { windowsHide: true, stdio: "pipe" }); };
    const put = (index: number, name: string, value: string): void => reg(["add", `HKCU\\${keys[index].key}`, "/v", name, "/t", "REG_SZ", "/d", value, "/f", "/reg:64"]);
    const check = (required: boolean): void => assertTestRegistration(readRegistryKeys(keys), install, required);
    const exercise = (): void => { try {
      if (failEarly) throw new Error("Failure before fixture key creation");
      writeFileSync(sentinel, "unchanged");
      expect(() => check(false)).not.toThrow();
      expect(() => { check(true); rmSync(sentinel); }).toThrow(/missing/i);
      put(0, "InstallLocation", root);
      put(1, "UninstallString", `"${join(install, `Uninstall ${testIdentity.product}.exe`)}" /currentuser`);
      expect(() => { check(true); rmSync(sentinel); }).toThrow(/InstallLocation/);
      put(0, "InstallLocation", install);
      put(1, "UninstallString", `"${join(install, "Uninstall Tjommie.exe")}" /currentuser`);
      expect(() => { check(true); rmSync(sentinel); }).toThrow(/UninstallString/);
      put(1, "UninstallString", `"${join(install, `Uninstall ${testIdentity.product}.exe`)}" /currentuser /D=C:\\wrong`);
      expect(() => check(true)).toThrow(/UninstallString/);
      put(1, "UninstallString", `"${join(install, `Uninstall ${testIdentity.product}.exe`)}" /currentuser`);
      expect(() => check(true)).not.toThrow();
      expect(readFileSync(sentinel, "utf8")).toBe("unchanged");
    } finally {
      if (!key.startsWith("Software\\TjommieGuardFixtures\\")) throw new Error("Unexpected fixture key.");
      spawnSync("reg.exe", ["delete", `HKCU\\${key}`, "/f", "/reg:64"], { windowsHide: true, stdio: "ignore" });
      expect(readRegistryKeys(keys).every((entry) => !entry.present)).toBe(true);
      if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Unexpected fixture directory.");
      rmSync(root, { recursive: true, force: true });
      expect(existsSync(root)).toBe(false);
    } };
    if (failEarly) expect(exercise).toThrow("Failure before fixture key creation");
    else exercise();
  }, 30_000);
});
