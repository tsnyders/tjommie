// Builds the Windows installer: npm run package. It lands in dist-desktop\installer, next to the unpacked app
// (win-unpacked) that the packaged smoke test runs.
import { execSync } from "node:child_process";
import { cpSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import builder from "electron-builder";
import { rolldown } from "rolldown";
import { buildInputHash, writePackagedProvenance } from "./packaged-provenance.ts";
import { prepareUiAssets } from "../../../scripts/prepare-ui.ts";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const require = createRequire(import.meta.url);
const testAppId = process.env.TJOMMIE_PACKAGE_APP_ID;
const testProduct = process.env.TJOMMIE_PACKAGE_PRODUCT;
const testDataRoot = process.env.TJOMMIE_PACKAGE_TEST_ROOT;
if ((testAppId || testProduct || testDataRoot || process.env.TJOMMIE_PACKAGE_VERSION) &&
  (testAppId !== "com.tsnyders.tjommie.updatetest" || testProduct !== "Tjommie Update Test")) {
  throw new Error("Update test packaging requires the isolated app identity.");
}
if (testAppId && (!testDataRoot || dirname(resolve(testDataRoot)) !== resolve(tmpdir()))) {
  throw new Error("Update test packaging requires a disposable data root.");
}
prepareUiAssets(repo);
const inputsSha256 = buildInputHash(repo);
const desktop = join(repo, "packages", "desktop");
const out = testDataRoot ? join(testDataRoot, "dist-desktop") : join(repo, "dist-desktop");
const app = join(out, "app"); // what goes into app.asar: the main process and the preload
const resources = join(out, "resources"); // plain files beside app.asar: the daemon, the UI and the icons
const appId = testAppId ?? "com.tsnyders.tjommie";
const productName = testProduct ?? "Tjommie";
const version = process.env.TJOMMIE_PACKAGE_VERSION ?? (require(join(repo, "package.json")) as { version: string }).version;
const electronVersion = (require("electron/package.json") as { version: string }).version;

// Node refuses to strip types under node_modules, so the installed app runs bundles, not the .ts sources.
async function bundle(input: string, dir: string, external: string[]): Promise<void> {
  const build = await rolldown({ input, platform: "node", external });
  await build.write({ dir, format: "esm", entryFileNames: "main.mjs", chunkFileNames: "[name]-[hash].mjs" });
  await build.close();
}

// Playwright MCP runs as its own process, so it ships as a package beside the daemon, with what it needs. These are
// the exact copies the daemon resolves in the workspace.
function copyPlaywrightMcp(target: string): void {
  const mcp = dirname(createRequire(join(repo, "packages", "daemon", "package.json")).resolve("@playwright/mcp/package.json"));
  const playwright = dirname(createRequire(join(mcp, "package.json")).resolve("playwright/package.json"));
  const core = dirname(createRequire(join(playwright, "package.json")).resolve("playwright-core/package.json"));
  const packages: [string, string][] = [
    ["@playwright/mcp", mcp],
    ["playwright", playwright],
    ["playwright-core", core],
  ];
  for (const [name, from] of packages) cpSync(from, join(target, "node_modules", name), { recursive: true });
}

rmSync(out, { recursive: true, force: true });
execSync("npm run build", { cwd: repo, stdio: "inherit", windowsHide: true }); // the static export

await bundle(join(desktop, "src", "main.ts"), join(app, "src"), ["electron"]);
cpSync(join(desktop, "src", "preload.cjs"), join(app, "src", "preload.cjs"));
writeFileSync(
  join(app, "package.json"),
  JSON.stringify({ name: testAppId ? "tjommie-updatetest" : "tjommie", appId, productName, version, ...(testDataRoot ? { testDataRoot } : {}), description: "AI bots on your own PC", author: "TSnyders", license: "Apache-2.0", type: "module", main: "src/main.mjs" }, null, 2),
);

// ws's optional native helpers stay out; ws uses its own JavaScript without them.
await bundle(join(repo, "packages", "daemon", "src", "main.ts"), join(resources, "daemon"), ["@playwright/mcp", "bufferutil", "utf-8-validate"]);
copyPlaywrightMcp(join(resources, "daemon"));
cpSync(join(repo, "packages", "transport", "wasm", "tjommie_noise_bg.wasm"), join(resources, "daemon", "tjommie_noise_bg.wasm"));
cpSync(join(repo, "packages", "ui", "out"), join(resources, "ui"), { recursive: true });
cpSync(join(desktop, "assets"), join(resources, "assets"), { recursive: true });

await builder.build({
  targets: builder.Platform.WINDOWS.createTarget("nsis", builder.Arch.x64),
  publish: "never",
  config: {
    appId,
    productName,
    artifactName: "Tjommie-Setup-${version}.${ext}",
    electronVersion,
    directories: { app, output: join(out, "installer") },
    extraResources: [{ from: resources, to: ".", filter: ["**/*"] }],
    asar: true,
    npmRebuild: false,
    win: { target: "nsis", icon: join(desktop, "assets", "icon-256.png") },
    // One click, for this Windows user only, no admin. Uninstalling keeps %LOCALAPPDATA%\Tjommie and the bots.
    nsis: { oneClick: true, perMachine: false, deleteAppDataOnUninstall: false },
    publish: [{ provider: "github", owner: "tsnyders", repo: "tjommie" }],
  },
});

// A successful build is verifiable only for these exact inputs and all installer/payload bytes.
if (!testAppId) writePackagedProvenance(repo, inputsSha256);
