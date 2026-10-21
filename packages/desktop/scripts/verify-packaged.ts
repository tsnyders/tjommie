// Mandatory Windows packaging gate: npm run package, then npm run test:packaged.
// This verifies the existing artifact; it never builds it or runs the other real CLI suites.
// Missing artifacts, missing test results, zero tests and skipped tests all fail the gate.
// Missing provenance or changed source/installer/payload bytes require a fresh `npm run package`.
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { packagedExecutable, verifyPackagedProvenance } from "./packaged-provenance.ts";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const testFile = join(repo, "packages", "desktop", "src", "packaged.real.test.ts");
const exe = join(repo, packagedExecutable);

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function verifyReport(value: unknown): void {
  if (!record(value) || value.success !== true || !Array.isArray(value.testResults) || value.testResults.length !== 1) {
    throw new Error("Packaged verification requires a successful result for exactly one test file.");
  }
  const result: unknown = value.testResults[0];
  if (!record(result) || typeof result.name !== "string" || resolve(result.name) !== testFile || result.status !== "passed") {
    throw new Error("The packaged smoke suite did not pass.");
  }
  const assertions: unknown = result.assertionResults;
  if (!Array.isArray(assertions) || assertions.length === 0 || !assertions.every((assertion: unknown) => record(assertion) && assertion.status === "passed")) {
    throw new Error("Packaged verification requires at least one test and every test must pass; skipped tests are not accepted.");
  }
  if (value.numTotalTests !== assertions.length || value.numPassedTests !== assertions.length || value.numPendingTests !== 0 || value.numFailedTests !== 0) {
    throw new Error("Packaged test counts do not confirm a complete passing run.");
  }
}

async function main(): Promise<void> {
  if (process.argv.length !== 2) throw new Error("test:packaged accepts no filters or overrides; run npm run test:packaged.");
  if (!existsSync(exe)) throw new Error("Packaged executable missing. Run npm run package before npm run test:packaged.");
  if (process.platform !== "win32") throw new Error("Packaged verification requires Windows.");
  verifyPackagedProvenance(repo);

  const reportDir = mkdtempSync(join(tmpdir(), "tjommie-packaged-report-"));
  const reportFile = join(reportDir, "results.json");
  try {
    const run = spawnSync(process.execPath, [
      join(repo, "node_modules", "vitest", "vitest.mjs"), "run",
      "--mode", "packaged", "--passWithNoTests=false",
      "--reporter=default", "--reporter=json", `--outputFile.json=${reportFile}`,
      testFile,
    ], { cwd: repo, stdio: "inherit" });
    if (run.error) throw run.error;
    if (run.status !== 0) throw new Error(`Packaged smoke failed (${run.signal ?? run.status ?? "no exit status"}).`);
    if (!existsSync(reportFile)) throw new Error("Packaged smoke produced no verification report.");
    const report: unknown = JSON.parse(readFileSync(reportFile, "utf8"));
    verifyReport(report);
    verifyPackagedProvenance(repo);
    console.log("Packaged verification passed: every packaged smoke test ran and passed.");
  } finally {
    rmSync(reportFile, { force: true });
    rmdirSync(reportDir);
  }
}

await main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : "Packaged verification failed.");
  process.exitCode = 1;
});
