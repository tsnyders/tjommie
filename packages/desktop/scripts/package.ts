// Builds the Windows installer: npm run package. It lands in dist-desktop\installer, next to the unpacked app
// (win-unpacked) that the packaged smoke test runs.
import { execSync } from "node:child_process";
import { appendFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import builder from "electron-builder";
import { rolldown } from "rolldown";
import { buildInputHash, writePackagedProvenance } from "./packaged-provenance.ts";
import { prepareUiAssets } from "../../../scripts/prepare-ui.ts";
import { assertTestDefines, assertTestInstallDir, assertTestInstaller, testIdentity, testNsisInclude } from "./update-test-guards.ts";
import { verifySpeechResources } from "../src/speech-resources.ts";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const require = createRequire(import.meta.url);
const testAppId = process.env.TJOMMIE_PACKAGE_APP_ID;
const testProduct = process.env.TJOMMIE_PACKAGE_PRODUCT;
const testDataRoot = process.env.TJOMMIE_PACKAGE_TEST_ROOT;
if ((testAppId || testProduct || testDataRoot || process.env.TJOMMIE_PACKAGE_VERSION) &&
  (testAppId !== testIdentity.appId || testProduct !== testIdentity.product)) {
  throw new Error("Update test packaging requires the isolated app identity.");
}
if (testAppId && (!testDataRoot || dirname(resolve(testDataRoot)) !== resolve(tmpdir()))) {
  throw new Error("Update test packaging requires a disposable data root.");
}
if (testDataRoot) assertTestInstallDir(join(testDataRoot, testIdentity.product));
prepareUiAssets(repo);
const inputsSha256 = buildInputHash(repo);
const desktop = join(repo, "packages", "desktop");
const out = testDataRoot ? join(testDataRoot, "dist-desktop") : join(repo, "dist-desktop");
const app = join(out, "app"); // what goes into app.asar: the main process and the preload
const resources = join(out, "resources"); // plain files beside app.asar: the daemon, the UI and the icons
// The Rust daemon, built before packaging (CI: `cargo build --release --locked -p daemon --bin tjommie-daemon` inside tjommie-rs).
const daemonExe = join(repo, "tjommie-rs", "target", "release", "tjommie-daemon.exe");
const appId = testAppId ?? "com.tsnyders.tjommie";
const productName = testProduct ?? "Tjommie";
const version = process.env.TJOMMIE_PACKAGE_VERSION ?? (require(join(repo, "package.json")) as { version: string }).version;
const electronVersion = (require("electron/package.json") as { version: string }).version;
const speechSource = join(repo, ".cache", "speech-runtime");
const speechResources = verifySpeechResources(speechSource);

// Node refuses to strip types under node_modules, so the installed app runs bundles, not the .ts sources.
async function bundle(input: string, dir: string, external: string[]): Promise<void> {
  const build = await rolldown({ input, platform: "node", external });
  await build.write({ dir, format: "esm", entryFileNames: "main.mjs", chunkFileNames: "[name]-[hash].mjs" });
  await build.close();
}

if (!existsSync(daemonExe)) {
  throw new Error("Build the Rust daemon before packaging: cargo build --release --locked -p daemon --bin tjommie-daemon, inside tjommie-rs.");
}

rmSync(out, { recursive: true, force: true });
execSync("npm run build", { cwd: repo, stdio: "inherit", windowsHide: true }); // the static export

await bundle(join(desktop, "src", "main.ts"), join(app, "src"), ["electron"]);
cpSync(join(desktop, "src", "preload.cjs"), join(app, "src", "preload.cjs"));
for (const name of ["vault-entry.html", "vault-entry.js", "vault-entry.css", "vault-entry-preload.cjs"]) cpSync(join(desktop, "src", name), join(app, "src", name));
writeFileSync(
  join(app, "package.json"),
  JSON.stringify({ name: testAppId ? testIdentity.packageName : "tjommie", appId, productName, version, ...(testDataRoot ? { testDataRoot } : {}), description: "AI bots on your own PC", author: "TSnyders", license: "Apache-2.0", type: "module", main: "src/main.mjs" }, null, 2),
);

// The daemon is one exe: no Node bundle, no Playwright MCP and no Noise wasm (the UI keeps its own copy of the wasm in resources/ui).
mkdirSync(join(resources, "daemon"), { recursive: true });
cpSync(daemonExe, join(resources, "daemon", "tjommie-daemon.exe"));
// The worker-acceptance tool in resources/vault still reads this helper from beside the daemon (..\daemon\windows-worker.ps1).
cpSync(join(repo, "packages", "daemon", "src", "windows-worker.ps1"), join(resources, "daemon", "windows-worker.ps1"));
cpSync(join(repo, "scripts", "vault"), join(resources, "vault"), { recursive: true });
await bundle(join(repo, "scripts", "vault", "accept-worker.ts"), join(resources, "vault", "acceptance"), []);
writeFileSync(join(resources, "daemon", "build-info.json"), JSON.stringify({ appVersion: version, daemonVersion: (require(join(repo, "packages", "daemon", "package.json")) as { version: string }).version }));
cpSync(join(repo, "packages", "ui", "out"), join(resources, "ui"), { recursive: true });
cpSync(join(desktop, "assets"), join(resources, "assets"), { recursive: true });
mkdirSync(join(resources, "speech"), { recursive: true });
for (const name of ["manifest.json", ...speechResources.files.map((file) => file.name)]) cpSync(join(speechSource, name), join(resources, "speech", name));
const speechNotices = speechResources.files.filter((file) => file.name !== "whisper-cli.exe");
if (speechNotices.length) appendFileSync(join(resources, "ui", "third-party-notices.txt"), "\n\nLocal speech\n\n" + speechNotices.map((file) => `${file.name}\n\n${readFileSync(join(speechSource, file.name), "utf8")}`).join("\n\n"));

const testInclude = testDataRoot ? join(out, "update-test-guard.nsh") : undefined;
if (testInclude && testDataRoot) writeFileSync(testInclude, testNsisInclude(join(testDataRoot, testIdentity.product)));
let testIdentityChecked = false;

await builder.build({
  targets: builder.Platform.WINDOWS.createTarget("nsis", builder.Arch.x64),
  publish: "never",
  ...(testAppId ? { effectiveOptionComputed: async (options: unknown): Promise<boolean> => {
    assertTestDefines(options);
    testIdentityChecked = true;
    return false;
  } } : {}),
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
    nsis: { oneClick: true, perMachine: false, deleteAppDataOnUninstall: false, ...(testInclude ? { include: testInclude } : {}) },
    publish: [{ provider: "github", owner: "tsnyders", repo: "tjommie" }],
  },
});

if (testAppId && !testIdentityChecked) throw new Error("Update-test NSIS identity gate did not run.");
if (testDataRoot) assertTestInstaller(join(out, "installer", `Tjommie-Setup-${version}.exe`), join(testDataRoot, testIdentity.product));

// A successful build is verifiable only for these exact inputs and all installer/payload bytes.
if (!testAppId) writePackagedProvenance(repo, inputsSha256);
