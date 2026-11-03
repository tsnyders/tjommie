import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const profile = process.env.USERPROFILE; assert(profile, "The existing Gradle dependency cache is unavailable");
const cache = join(profile, ".gradle", "caches", "modules-2", "files-2.1");
function jar(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name); if (entry.isFile() && entry.name.endsWith(".jar")) return path;
    if (entry.isDirectory()) { const found = jar(path); if (found) return found; }
  }
  return null;
}
const junit = jar(join(cache, "junit", "junit", "4.13.2")), hamcrest = jar(join(cache, "org.hamcrest", "hamcrest-core", "1.3"));
assert(junit && hamcrest, "The existing JUnit dependencies are unavailable");
const root = mkdtempSync(join(tmpdir(), "tjommie-attachments-jvm-"));
try {
  const sources = ["AttachmentFileStore", "AttachmentInputStream", "AttachmentRanges", "StaticRoutes"].map((name) => join(repository, "packages/mobile/native/java/dev/tjommie/app", `${name}.java`));
  sources.push(join(repository, "packages/mobile/native/java-test/dev/tjommie/app/AttachmentCoreTest.java"));
  sources.push(join(repository, "packages/mobile/native/java-test/dev/tjommie/app/StaticRoutesTest.java"));
  const compile = spawnSync("javac", ["-d", root, "-cp", [junit, hamcrest].join(delimiter), ...sources], { encoding: "utf8", windowsHide: true });
  assert.equal(compile.status, 0, `JVM core test compilation failed: ${compile.stderr}`);
  const test = spawnSync("java", ["-cp", [root, junit, hamcrest].join(delimiter), "org.junit.runner.JUnitCore", "dev.tjommie.app.AttachmentCoreTest", "dev.tjommie.app.StaticRoutesTest"], { encoding: "utf8", windowsHide: true });
  process.stdout.write(test.stdout); assert.equal(test.status, 0, `JVM core test failed: ${test.stderr}`);
} finally {
  assert.equal(dirname(resolve(root)), resolve(tmpdir())); assert(root.startsWith(join(resolve(tmpdir()), "tjommie-attachments-jvm-")));
  rmSync(root, { recursive: true, force: true });
}
