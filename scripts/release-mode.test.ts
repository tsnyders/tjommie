import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { releaseMode } from "./release-mode.ts";

// T8.3: whether a run of release.yml may publish. Only a push of a v* tag does. Anything else, however it was started, is a dry run that
// builds and uploads workflow artifacts and creates no release. The default for anything unexpected is the dry run.

const script = join(import.meta.dirname, "release-mode.ts");
const folders: string[] = [];
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

describe("the release mode", () => {
  it("publishes only for a push of a v* tag", () => {
    expect(releaseMode("push", "refs/tags/v0.6.0")).toEqual({ dryRun: false, reason: "a push of the tag refs/tags/v0.6.0" });
    expect(releaseMode("push", "refs/tags/v1").dryRun).toBe(false);
  });

  it.each([
    ["a manual dispatch, even on a tag", "workflow_dispatch", "refs/tags/v0.6.0"],
    ["a manual dispatch on a branch", "workflow_dispatch", "refs/heads/rs3/release-wf"],
    ["a push to a dry-run branch", "push", "refs/heads/release-dryrun-1"],
    ["a push to main", "push", "refs/heads/main"],
    ["a push of a tag that is not a v* tag", "push", "refs/tags/x1"],
    ["a push of a tag named like a branch", "push", "refs/heads/v0.6.0"],
    ["a pull request", "pull_request", "refs/pull/1/merge"],
    ["a schedule", "schedule", "refs/heads/main"],
    ["a v tag on an event that is not a push", "release", "refs/tags/v0.6.0"],
  ])("is a dry run for %s", (_label, event, ref) => {
    expect(releaseMode(event, ref).dryRun).toBe(true);
  });

  it("is a dry run when it cannot tell: no event, no ref, or something odd", () => {
    expect(releaseMode(undefined, undefined).dryRun).toBe(true);
    expect(releaseMode("push", undefined).dryRun).toBe(true);
    expect(releaseMode(undefined, "refs/tags/v0.6.0").dryRun).toBe(true);
    expect(releaseMode("", "").dryRun).toBe(true);
    expect(releaseMode("PUSH", "refs/tags/v0.6.0").dryRun).toBe(true); // case matters: GitHub's value is "push"
    expect(releaseMode("push", "refs/tags/v0.6.0\nrefs/heads/main").dryRun).toBe(true);
    expect(releaseMode("push", " refs/tags/v0.6.0").dryRun).toBe(true);
  });

  // The command writes what GitHub Actions reads from $GITHUB_OUTPUT.
  it("writes dry_run and the package.json version as step outputs, from the environment GitHub gives a step", () => {
    const dir = mkdtempSync(join(tmpdir(), "tjommie-mode-test-"));
    folders.push(dir);
    const output = join(dir, "output.txt");
    writeFileSync(output, "");
    const run = (event: string, ref: string): string => {
      writeFileSync(output, "");
      const result = spawnSync(process.execPath, [script], { encoding: "utf8", windowsHide: true, env: { ...process.env, GITHUB_EVENT_NAME: event, GITHUB_REF: ref, GITHUB_OUTPUT: output } });
      expect(result.status, result.stderr).toBe(0);
      return readFileSync(output, "utf8");
    };
    const version = (JSON.parse(readFileSync(resolve(import.meta.dirname, "..", "package.json"), "utf8")) as { version: string }).version;
    expect(run("push", "refs/tags/v0.6.0").split("\n").filter(Boolean)).toEqual(["dry_run=false", `version=${version}`]);
    expect(run("workflow_dispatch", "refs/heads/x").split("\n").filter(Boolean)).toEqual(["dry_run=true", `version=${version}`]);
    expect(run("push", "refs/heads/release-dryrun-a").split("\n").filter(Boolean)).toEqual(["dry_run=true", `version=${version}`]);
  });

  it("is a dry run, and says so, when it is run with nothing set", () => {
    const env = { ...process.env };
    delete env.GITHUB_EVENT_NAME;
    delete env.GITHUB_REF;
    delete env.GITHUB_OUTPUT;
    const result = spawnSync(process.execPath, [script], { encoding: "utf8", windowsHide: true, env });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("dry_run=true");
  });
});
