import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

const repo = resolve(import.meta.dirname, "../../..");
function cachedJar(group, artifact, version) {
  const root = join(homedir(), ".gradle/caches/modules-2/files-2.1", group, artifact, version);
  if (existsSync(root)) for (const hash of readdirSync(root)) {
    const path = join(root, hash, `${artifact}-${version}.jar`);
    if (existsSync(path)) return path;
  }
  throw new Error(`The cached ${artifact} ${version} test dependency is missing.`);
}
const junit = cachedJar("junit", "junit", "4.13.2");
const hamcrest = cachedJar("org.hamcrest", "hamcrest-core", "1.3");
const scratch = mkdtempSync(join(tmpdir(), "tjommie-speech-jvm-"));
const binary = (name) => process.env.JAVA_HOME ? join(process.env.JAVA_HOME, "bin", `${name}${process.platform === "win32" ? ".exe" : ""}`) : name;
try {
  execFileSync(binary("javac"), ["-d", scratch, "-cp", junit, "packages/mobile/native/java/dev/tjommie/app/SpeechSession.java", "packages/mobile/native/java-test/dev/tjommie/app/SpeechSessionTest.java"], { cwd: repo, windowsHide: true, stdio: "inherit" });
  execFileSync(binary("java"), ["-cp", [scratch, junit, hamcrest].join(process.platform === "win32" ? ";" : ":"), "org.junit.runner.JUnitCore", "dev.tjommie.app.SpeechSessionTest"], { cwd: repo, windowsHide: true, stdio: "inherit" });
} finally {
  assert.equal(dirname(resolve(scratch)), resolve(tmpdir()));
  assert(basename(scratch).startsWith("tjommie-speech-jvm-"));
  rmSync(scratch, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
