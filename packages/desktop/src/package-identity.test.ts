import { expect, it, vi } from "vitest";
import builder from "electron-builder";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testIdentity } from "../scripts/update-test-guards.ts";

vi.mock("node:child_process", async (original) => ({ ...await original<typeof import("node:child_process")>(), execSync: vi.fn() }));
vi.mock("node:fs", async (original) => ({ ...await original<typeof import("node:fs")>(), cpSync: vi.fn(), rmSync: vi.fn(), writeFileSync: vi.fn() }));
vi.mock("electron-builder", () => ({ default: { build: vi.fn(), Platform: { WINDOWS: { createTarget: vi.fn() } }, Arch: { x64: 1 } } }));
vi.mock("rolldown", () => ({ rolldown: async () => ({ write: vi.fn(), close: vi.fn() }) }));
vi.mock("../../../scripts/prepare-ui.ts", () => ({ prepareUiAssets: vi.fn() }));
vi.mock("../scripts/packaged-provenance.ts", () => ({ buildInputHash: () => "fixture", writePackagedProvenance: vi.fn() }));

it("leaves production packaging options unchanged, with no test hook or NSIS include", async () => {
  for (const key of ["TJOMMIE_PACKAGE_APP_ID", "TJOMMIE_PACKAGE_PRODUCT", "TJOMMIE_PACKAGE_TEST_ROOT", "TJOMMIE_PACKAGE_VERSION"]) vi.stubEnv(key, undefined);
  try {
    await import("../scripts/package.ts");
    const options = vi.mocked(builder.build).mock.calls[0]?.[0];
    expect(options?.effectiveOptionComputed).toBeUndefined();
    const config = options?.config;
    if (!config || typeof config !== "object") throw new Error("Expected inline production configuration.");
    expect(config.appId).toBe("com.tsnyders.tjommie");
    expect(config.productName).toBe("Tjommie");
    expect(config.nsis).toEqual({ oneClick: true, perMachine: false, deleteAppDataOnUninstall: false });
    const metadata = vi.mocked(writeFileSync).mock.calls.find(([file]) => String(file).endsWith("package.json"))?.[1];
    expect(JSON.parse(String(metadata))).toMatchObject({ name: "tjommie", appId: "com.tsnyders.tjommie", productName: "Tjommie" });
  } finally { vi.unstubAllEnvs(); }
});

it("fails test packaging if electron-builder stops invoking the computed identity gate", async () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-package-hook-"));
  vi.stubEnv("TJOMMIE_PACKAGE_APP_ID", testIdentity.appId);
  vi.stubEnv("TJOMMIE_PACKAGE_PRODUCT", testIdentity.product);
  vi.stubEnv("TJOMMIE_PACKAGE_TEST_ROOT", root);
  vi.stubEnv("TJOMMIE_PACKAGE_VERSION", "0.3.1-test");
  try {
    vi.resetModules();
    await expect(import("../scripts/package.ts")).rejects.toThrow(/identity gate did not run/);
  } finally {
    vi.unstubAllEnvs();
    const fs = await vi.importActual<typeof import("node:fs")>("node:fs");
    fs.rmSync(root, { recursive: true, force: true });
  }
});
