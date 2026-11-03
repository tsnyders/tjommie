import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const script = fileURLToPath(new URL("./apply-native.mjs", import.meta.url));

it("accepts a mapped Android release version without applying native changes", () => {
  const result = spawnSync(process.execPath, [script, "--check", "0.3.2"], { encoding: "utf8" });
  expect(result.status).toBe(0);
  expect(result.stdout).toContain("Android versionCode mapping found for 0.3.2.");
});

it("rejects a release version without a versionCode", () => {
  const result = spawnSync(process.execPath, [script, "--check", "0.3.99"], { encoding: "utf8" });
  expect(result.status).not.toBe(0);
  expect(result.stderr).toContain("Android versionCode needs a release-version rule for 0.3.99.");
});

it.each(["", '<queries><intent><action android:name="android.intent.action.VIEW" /></intent></queries>'])("applies speech permissions, service visibility and source copies idempotently with queries %s", (queries) => {
  const scratch = mkdtempSync(join(tmpdir(), "tjommie-native-apply-"));
  const mobile = join(scratch, "packages", "mobile"), source = resolve(dirname(script), "..");
  const main = join(mobile, "android", "app", "src", "main");
  const write = (path: string, content: string) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, content); };
  try {
    for (const name of ["apply-native.mjs", "firebase-dependency.ts"]) {
      mkdirSync(join(mobile, "scripts"), { recursive: true }); cpSync(join(source, "scripts", name), join(mobile, "scripts", name));
    }
    cpSync(join(source, "native"), join(mobile, "native"), { recursive: true });
    write(join(mobile, "package.json"), JSON.stringify({ type: "module", version: "0.3.2" }));
    write(join(scratch, "packages", "desktop", "assets", "icon-256.png"), "synthetic icon");
    mkdirSync(join(main, "res", "mipmap-anydpi-v26"), { recursive: true });
    write(join(main, "res", "values", "styles.xml"), '<resources><style><item name="windowNoTitle">true</item></style></resources>');
    write(join(main, "AndroidManifest.xml"), `<manifest xmlns:android="http://schemas.android.com/apk/res/android">${queries}<application><activity /></application></manifest>`);
    write(join(mobile, "android", "variables.gradle"), "ext { firebaseMessagingVersion = '25.0.1' }");
    write(join(mobile, "android", "app", "build.gradle"), 'android {\n    defaultConfig { versionCode 1\n versionName "1.0" }\n    buildTypes { release {} }\n}\ndependencies {\n}');
    for (let run = 0; run < 2; run++) {
      const result = spawnSync(process.execPath, [join(mobile, "scripts", "apply-native.mjs")], { encoding: "utf8", env: { ...process.env, TJOMMIE_GOOGLE_SERVICES_JSON: "" } });
      expect(result.status, result.stderr).toBe(0);
    }
    const manifest = readFileSync(join(main, "AndroidManifest.xml"), "utf8");
    expect(manifest.match(/android\.permission\.RECORD_AUDIO/g)).toHaveLength(1);
    expect(manifest.match(/android\.speech\.RecognitionService/g)).toHaveLength(1);
    expect(manifest.match(/<queries>/g)).toHaveLength(1);
    const gradle = readFileSync(join(mobile, "android", "app", "build.gradle"), "utf8");
    expect(gradle.match(/applicationIdSuffix "\.phase4test"/g)).toHaveLength(1);
    expect(gradle.match(/manifestPlaceholders = \[appLabel: "Tjommie"\]/g)).toHaveLength(1);
    if (queries) expect(manifest).toContain('android:name="android.intent.action.VIEW"');
    for (const name of ["MainActivity", "StaticWebViewClient", "TjommieAttachmentPlugin", "TjommieSpeechPlugin"]) {
      const file = join("java", "dev", "tjommie", "app", `${name}.java`);
      expect(readFileSync(join(main, file), "utf8")).toBe(readFileSync(join(source, "native", file), "utf8"));
    }
    const test = join("java", "dev", "tjommie", "app", "AttachmentCoreTest.java");
    expect(readFileSync(join(mobile, "android", "app", "src", "test", test), "utf8")).toBe(readFileSync(join(source, "native", "java-test", "dev", "tjommie", "app", "AttachmentCoreTest.java"), "utf8"));
  } finally {
    expect(dirname(resolve(scratch))).toBe(resolve(tmpdir()));
    expect(scratch).toContain("tjommie-native-apply-"); rmSync(scratch, { recursive: true, force: true });
  }
});
