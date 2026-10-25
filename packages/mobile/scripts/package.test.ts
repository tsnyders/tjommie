import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const scripts = dirname(fileURLToPath(import.meta.url));
const mobile = join(scripts, "..");
const repo = join(mobile, "../..");

it("refuses release packaging before building when signing variables are missing", () => {
  const env = { ...process.env };
  for (const name of ["TJOMMIE_ANDROID_KEYSTORE", "TJOMMIE_ANDROID_KEYSTORE_PASSWORD", "TJOMMIE_ANDROID_KEY_ALIAS", "TJOMMIE_ANDROID_KEY_PASSWORD"]) delete env[name];
  const result = spawnSync(process.execPath, [join(scripts, "package.ts"), "--release"], { cwd: repo, env, encoding: "utf8", windowsHide: true });
  expect(result.status).not.toBe(0);
  expect(result.stderr).toContain("Release signing requires all four TJOMMIE_ANDROID_* variables and a keystore file.");
});

it("attaches only release signing and gates release artifacts with environment values", () => {
  const gradle = readFileSync(join(mobile, "android/app/build.gradle"), "utf8");
  expect(gradle).toMatch(/buildTypes\s*\{\s*release\s*\{\s*signingConfig signingConfigs\.release/);
  expect(gradle).not.toMatch(/signingConfig signingConfigs\.debug/);
  expect(gradle).toContain('tasks.register("requireReleaseSigning")');
  expect(gradle).toContain('tasks.matching { it.name in ["packageRelease", "packageReleaseUniversalApk", "signReleaseBundle"] }.configureEach { dependsOn(tasks.named("requireReleaseSigning")) }');
  expect(gradle).not.toContain('it.name == "preReleaseBuild"');
  expect(gradle.match(/^\s*(?:storeFile|storePassword|keyAlias|keyPassword)\b.*$/gm)?.map((line) => line.trim())).toEqual([
    'storeFile System.getenv("TJOMMIE_ANDROID_KEYSTORE") ? file(System.getenv("TJOMMIE_ANDROID_KEYSTORE")) : null',
    'storePassword System.getenv("TJOMMIE_ANDROID_KEYSTORE_PASSWORD")',
    'keyAlias System.getenv("TJOMMIE_ANDROID_KEY_ALIAS")',
    'keyPassword System.getenv("TJOMMIE_ANDROID_KEY_PASSWORD")',
  ]);
  expect(gradle).toMatch(/if \(names\.any \{ !System\.getenv\(it\) \}/);
});
