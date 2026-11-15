import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { releaseMode } from "./release-mode.ts";

// T8.3: the release workflow has a dry run that cannot publish. GitHub only runs a workflow on a runner, so this file is how it is proved
// here: every workflow is parsed with a real YAML parser (GitHub refuses a bad one before any job starts, with no log), and release.yml is
// then walked as GitHub would walk it, for each kind of event, to show that outside a push of a v* tag no step creates a release, pushes a
// tag or touches a latest.yml, and that the one job allowed to do so is skipped.

vi.setConfig({ testTimeout: 60_000 });

const repo = resolve(import.meta.dirname, "..");
const workflowDir = join(repo, ".github", "workflows");
const parser = join(import.meta.dirname, "parse-yaml.mjs");

type Step = {
  name?: string;
  id?: string;
  uses?: string;
  run?: string;
  if?: string | boolean;
  shell?: string;
  env?: Record<string, string>;
  with?: Record<string, string | number | boolean>;
  "working-directory"?: string;
  "continue-on-error"?: boolean;
};
type Job = {
  needs?: string | string[];
  if?: string | boolean;
  permissions?: string | Record<string, string>;
  "runs-on"?: string;
  "timeout-minutes"?: number;
  outputs?: Record<string, string>;
  env?: Record<string, string>;
  steps?: Step[];
};
type Workflow = {
  name?: string;
  on?: Record<string, unknown>;
  permissions?: string | Record<string, string>;
  jobs?: Record<string, Job>;
};

function parseWorkflow(file: string): Workflow {
  const result = spawnSync(process.execPath, [parser, file], { encoding: "utf8", windowsHide: true });
  if (result.status !== 0) throw new Error(`${file} is not valid YAML: ${result.stderr}`);
  return JSON.parse(result.stdout) as Workflow;
}

const folders: string[] = [];
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

describe("every workflow", () => {
  const files = readdirSync(workflowDir).filter((name) => /\.ya?ml$/.test(name)).sort();

  it("finds the workflows", () => {
    expect(files).toContain("release.yml");
  });

  it.each(files)("%s is valid YAML shaped like a workflow", (name) => {
    const workflow = parseWorkflow(join(workflowDir, name));
    expect(typeof workflow.name, "a name").toBe("string");
    expect(workflow.on, "a trigger").toBeTruthy();
    const jobs = workflow.jobs ?? {};
    expect(Object.keys(jobs).length, "at least one job").toBeGreaterThan(0);
    for (const [jobName, job] of Object.entries(jobs)) {
      expect(job["runs-on"], `${jobName} runs on something`).toBeTruthy();
      expect(Array.isArray(job.steps) && job.steps.length > 0, `${jobName} has steps`).toBe(true);
      const needs = typeof job.needs === "string" ? [job.needs] : job.needs ?? [];
      for (const need of needs) expect(Object.keys(jobs), `${jobName} needs a job that exists`).toContain(need);
      for (const step of job.steps ?? []) expect(Boolean(step.run) !== Boolean(step.uses), `${jobName}: "${step.name ?? step.uses}" has run or uses, not both`).toBe(true);
    }
  });
});

describe("release.yml", () => {
  const workflow = parseWorkflow(join(workflowDir, "release.yml"));
  const jobs = workflow.jobs ?? {};
  const jobNames = Object.keys(jobs);
  const stepsOf = (name: string): Step[] => jobs[name]?.steps ?? [];
  const needsOf = (job: Job): string[] => (typeof job.needs === "string" ? [job.needs] : job.needs ?? []);
  const stepText = (step: Step): string => [step.run ?? "", step.uses ?? "", JSON.stringify(step.with ?? {}), JSON.stringify(step.env ?? {})].join("\n");
  const everyStep = (): { job: string; step: Step }[] => jobNames.flatMap((job) => stepsOf(job).map((step) => ({ job, step })));
  const findStep = (job: string, pattern: RegExp): Step => {
    const found = stepsOf(job).filter((step) => pattern.test(`${step.name ?? ""}\n${step.run ?? ""}\n${step.uses ?? ""}`));
    expect(found.length, `${job}: exactly one step matches ${pattern}`).toBe(1);
    return found[0];
  };

  // Anything that creates a release, moves a tag or code, calls the GitHub API or posts to a service. Only the release job may have these.
  const PUBLISHING: RegExp[] = [
    /\bgh\s/,
    // merge-base only reads history (the verify job asks whether the tag is on main), so "merge" must not be followed by "-base".
    /\bgit\s+(push|tag|commit|remote|config|am|cherry-pick|merge(?![\w-]))/,
    /--verify-tag/,
    /\b(curl|wget|Invoke-WebRequest|Invoke-RestMethod|iwr|irm)\b/i,
    /\b(npm|cargo|yarn|pnpm)\s+publish\b/,
    /\bwrangler\b/i,
  ];
  const ALLOWED_ACTIONS = ["actions/checkout", "actions/setup-node", "actions/setup-java", "actions/upload-artifact"];

  it("starts on a v* tag, a release-dryrun-* branch or a manual dispatch, and on nothing else", () => {
    expect(workflow.on).toEqual({ push: { tags: ["v*"], branches: ["release-dryrun-*"] }, workflow_dispatch: null });
  });

  it("is read-only by default and gives write access to the release job alone", () => {
    expect(workflow.permissions).toEqual({ contents: "read" });
    for (const [name, job] of Object.entries(jobs)) {
      if (name === "release") continue;
      const granted = typeof job.permissions === "object" ? Object.values(job.permissions) : [];
      expect(job.permissions === undefined || (typeof job.permissions === "object" && granted.every((level) => level === "read" || level === "none")), `${name} has no write permission`).toBe(true);
    }
    expect(jobs.release?.permissions).toEqual({ contents: "write" });
  });

  it("lists every job after the jobs it needs (the walk below depends on it)", () => {
    jobNames.forEach((name, index) => {
      for (const need of needsOf(jobs[name])) expect(jobNames.indexOf(need), `${name} comes after ${need}`).toBeLessThan(index);
    });
    expect(jobNames).toEqual(["verify", "windows", "android", "release"]);
  });

  it("hands the mode from the verify job to the others through step outputs", () => {
    expect(jobs.verify.outputs).toEqual({ dry_run: "${{ steps.mode.outputs.dry_run }}", version: "${{ steps.mode.outputs.version }}" });
    const mode = findStep("verify", /Decide whether this run may publish/);
    expect(mode.id).toBe("mode");
    expect(mode.run).toContain("node scripts/release-mode.ts");
    expect(mode.run).toContain('>> "$GITHUB_OUTPUT"');
    expect(jobs.windows.env).toEqual({ DRY_RUN: "${{ needs.verify.outputs.dry_run }}", VERSION: "${{ needs.verify.outputs.version }}" });
  });

  describe("the publish job", () => {
    const release = jobs.release;

    it("needs every build, and runs only for a push of a v* tag that the mode agreed was a release", () => {
      expect(needsOf(release).sort()).toEqual(["android", "verify", "windows"]);
      const condition = String(release.if);
      expect(condition).toContain("needs.verify.outputs.dry_run == 'false'");
      expect(condition).toContain("github.event_name == 'push'");
      expect(condition).toContain("startsWith(github.ref, 'refs/tags/v')");
      expect(condition.split("&&").length, "the three conditions are ANDed").toBe(3);
      expect(condition).not.toMatch(/\|\||always\(\)|cancelled\(\)|failure\(\)/);
    });

    it("checks the tag push again inside the step, before it creates anything", () => {
      const publish = findStep("release", /Publish GitHub release/);
      const script = publish.run ?? "";
      const guard = script.indexOf('"$GITHUB_EVENT_NAME" != "push"');
      expect(guard, "the guard").toBeGreaterThanOrEqual(0);
      expect(script).toContain('"$GITHUB_REF" != refs/tags/v*');
      expect(script.indexOf("exit 1", guard)).toBeGreaterThan(guard);
      expect(script.indexOf("gh release create")).toBeGreaterThan(script.indexOf("exit 1", guard));
      expect(script).toContain("--verify-tag");
    });

    it("publishes the installer, latest.yml and the APK, with no block map", () => {
      const script = findStep("release", /Publish GitHub release/).run ?? "";
      for (const asset of ['"release-assets/Tjommie-Setup-$version.exe"', "release-assets/latest.yml", '"release-assets/Tjommie-Android-$version.apk"']) expect(script).toContain(asset);
      expect(script).not.toMatch(/blockmap/i);
    });

    it("only downloads artifacts, apart from the publishing step", () => {
      const others = stepsOf("release").filter((step) => !/Publish GitHub release/.test(step.name ?? ""));
      for (const step of others) expect(step.uses ?? "", `${step.name ?? step.uses}`).toMatch(/^actions\/download-artifact@[0-9a-f]{40}\b/);
      expect(others.map((step) => step.with?.name).sort()).toEqual(["android-assets", "release-notes", "windows-assets"]);
    });
  });

  describe("outside the publish job", () => {
    const others = jobNames.filter((name) => name !== "release");

    it("has no step that creates a release, moves a tag or code, or calls an API", () => {
      for (const { job, step } of everyStep()) {
        if (job === "release") continue;
        for (const pattern of PUBLISHING) expect(step.run ?? "", `${job}: "${step.name ?? step.uses}" must not match ${pattern}`).not.toMatch(pattern);
      }
    });

    it("uses only the plain build actions, each pinned to a commit", () => {
      for (const { job, step } of everyStep()) {
        if (!step.uses) continue;
        expect(step.uses, `${job}: ${step.uses} is pinned`).toMatch(/^[\w.-]+\/[\w.-]+@[0-9a-f]{40}( |$)/);
        if (job !== "release") expect(ALLOWED_ACTIONS, `${job} may use ${step.uses}`).toContain(step.uses.split("@")[0]);
      }
    });

    it("never sees the workflow token, and leaves no credentials in the checkout", () => {
      for (const name of others) expect(JSON.stringify(jobs[name]), `${name} has no token`).not.toMatch(/github\.token|GITHUB_TOKEN|secrets\.GITHUB/);
      for (const { step } of everyStep()) if (step.uses?.startsWith("actions/checkout@")) expect(step.with?.["persist-credentials"]).toBe(false);
    });
  });

  // GitHub's own rules, applied by hand: a job runs when it has an `if` that holds and every job it needs ran; a step runs when its `if` holds.
  // Only the forms this workflow uses are understood, so an expression the walk cannot evaluate fails the test and does not pass it by accident.
  type Context = { dryRun: string; event: string; ref: string };
  const operand = (token: string, context: Context): string => {
    const literal = /^'([^']*)'$/.exec(token.trim());
    if (literal) return literal[1];
    switch (token.trim()) {
      case "needs.verify.outputs.dry_run":
      case "steps.mode.outputs.dry_run":
        return context.dryRun;
      case "github.event_name":
        return context.event;
      case "github.ref":
        return context.ref;
      default:
        throw new Error(`The test cannot evaluate "${token}"; teach it before using that in an if.`);
    }
  };
  const atom = (source: string, context: Context): boolean => {
    const text = source.trim();
    if (text === "always()") return true;
    const prefix = /^startsWith\(\s*([^,]+?)\s*,\s*('[^']*')\s*\)$/.exec(text);
    if (prefix) return operand(prefix[1], context).startsWith(operand(prefix[2], context));
    const comparison = /^(\S+)\s*(==|!=)\s*('[^']*'|\S+)$/.exec(text);
    if (comparison) return (operand(comparison[1], context) === operand(comparison[3], context)) === (comparison[2] === "==");
    throw new Error(`The test cannot evaluate "${text}".`);
  };
  const holds = (condition: string | boolean | undefined, context: Context): boolean => {
    if (condition === undefined) return true;
    if (typeof condition === "boolean") return condition;
    const inner = condition.trim().replace(/^\$\{\{\s*([\s\S]*?)\s*\}\}$/, "$1");
    if (/\|\||!\(/.test(inner)) throw new Error(`The test cannot evaluate "${inner}": no || or negated groups.`);
    return inner.split("&&").every((part) => atom(part, context));
  };
  const walk = (context: Context): { jobs: string[]; steps: { job: string; step: Step }[] } => {
    const ran: string[] = [];
    const steps: { job: string; step: Step }[] = [];
    for (const name of jobNames) {
      if (!needsOf(jobs[name]).every((need) => ran.includes(need)) || !holds(jobs[name].if, context)) continue;
      ran.push(name);
      for (const step of stepsOf(name)) if (holds(step.if, context)) steps.push({ job: name, step });
    }
    return { jobs: ran, steps };
  };
  const contextFor = (event: string, ref: string, dryRun?: string): Context => ({ event, ref, dryRun: dryRun ?? String(releaseMode(event, ref).dryRun) });
  const uploads = (steps: { job: string; step: Step }[]): string[] =>
    steps.filter(({ step }) => step.uses?.startsWith("actions/upload-artifact@")).map(({ step }) => String(step.with?.name)).sort();

  describe("a dry run, walked the way GitHub walks it", () => {
    const dryRuns: [string, string, string][] = [
      ["a manual dispatch on a branch", "workflow_dispatch", "refs/heads/rs3/release-wf"],
      ["a manual dispatch on a v* tag", "workflow_dispatch", "refs/tags/v0.6.0"],
      ["a push to a release-dryrun branch", "push", "refs/heads/release-dryrun-1"],
      ["a push to main", "push", "refs/heads/main"],
      ["a push of a tag that is not v*", "push", "refs/tags/x1"],
      ["an event nobody listed", "schedule", "refs/heads/main"],
    ];

    it.each(dryRuns)("%s builds and checks, publishes nothing, and uploads only the dry-run artifact", (_label, event, ref) => {
      const context = contextFor(event, ref);
      expect(context.dryRun).toBe("true");
      const run = walk(context);
      expect(run.jobs).toEqual(["verify", "windows"]);
      expect(uploads(run.steps)).toEqual(["release-dryrun-windows"]);
      for (const { job, step } of run.steps) {
        for (const pattern of PUBLISHING) expect(step.run ?? "", `${job}: "${step.name ?? step.uses}" ran in a dry run and matches ${pattern}`).not.toMatch(pattern);
        if (step.uses) expect(ALLOWED_ACTIONS).toContain(step.uses.split("@")[0]);
      }
      const names = run.steps.map(({ step }) => step.name ?? "");
      expect(names).not.toContain("Check tag and extract release notes");
      expect(names).toContain("Check the Android version map (a dry run only reports it)");
      expect(names).toContain("Write the release manifest");
    });

    it("never builds the signed Android APK, which needs the release secrets", () => {
      for (const [, event, ref] of dryRuns) expect(walk(contextFor(event, ref)).jobs).not.toContain("android");
    });

    it("publishes nothing even if the mode output is lost, and then asks for the strict payload", () => {
      for (const lost of ["", "maybe", "TRUE"]) {
        const run = walk(contextFor("push", "refs/tags/v0.6.0", lost));
        expect(run.jobs, `dry_run is "${lost}"`).not.toContain("release");
        expect(run.jobs).not.toContain("android");
        expect(uploads(run.steps)).not.toContain("windows-assets");
      }
      const assemble = findStep("windows", /assemble-payload\.ps1/).run ?? "";
      expect(assemble).toContain("if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }");
    });

    it("has no way into the publish job from a dry run, whatever else is true", () => {
      for (const event of ["push", "workflow_dispatch", "schedule", "release", "pull_request"]) {
        for (const ref of ["refs/tags/v0.6.0", "refs/heads/main", "refs/heads/release-dryrun-9"]) {
          const context = contextFor(event, ref, "true");
          expect(holds(jobs.release.if, context), `${event} on ${ref} with dry_run true`).toBe(false);
        }
      }
    });
  });

  it("publishes only for a push of a v* tag, with everything it needs", () => {
    const context = contextFor("push", "refs/tags/v0.6.0");
    expect(context.dryRun).toBe("false");
    const run = walk(context);
    expect(run.jobs).toEqual(["verify", "windows", "android", "release"]);
    expect(uploads(run.steps)).toEqual(["android-assets", "release-notes", "windows-assets"]);
    expect(run.steps.map(({ step }) => step.name ?? "")).toContain("Publish GitHub release");
  });

  it("is the only way the publish job runs: every other event and ref skips it", () => {
    const others: [string, string][] = [["workflow_dispatch", "refs/tags/v0.6.0"], ["push", "refs/heads/v0.6.0"], ["push", "refs/tags/0.6.0"], ["release", "refs/tags/v0.6.0"]];
    for (const [event, ref] of others) expect(walk(contextFor(event, ref)).jobs, `${event} on ${ref}`).not.toContain("release");
  });

  describe("the Windows job", () => {
    const windows = (): Step[] => stepsOf("windows");

    it("builds the production desktop app and the daemon from the lock file, with no test driver", () => {
      const build = findStep("windows", /cargo build/);
      expect(build["working-directory"]).toBe("tjommie-rs");
      expect(build.env).toEqual({ TJOMMIE_IDENTITY: "production" });
      expect(build.run).toBe("cargo build --release --locked -p daemon -p desktop");
    });

    it("runs its steps in the order that makes each check meaningful", () => {
      const order: RegExp[] = [
        /npm run typecheck/,
        /build-speech-runtime\.ps1/,
        /cargo build --release --locked/,
        /check-production-binaries\.ps1/,
        /check-release-version\.ts/,
        /assemble-payload\.ps1/,
        /choco install nsis --version=3\.11/,
        /\.\/installer\/build-installer\.ps1/,
        /make-latest-yml\.ts/,
        /check-release-installer\.ps1/,
        /write-release-manifest\.ps1/,
        /upload-artifact[\s\S]*windows-assets/,
        /upload-artifact[\s\S]*release-dryrun-windows/,
      ];
      const indexes = order.map((pattern) => {
        const matching = windows().map((step, index) => ({ step, index })).filter(({ step }) => pattern.test(`${step.run ?? ""}\n${step.uses ?? ""}\n${JSON.stringify(step.with ?? {})}`));
        expect(matching.length, `one step matches ${pattern}`).toBe(1);
        return matching[0].index;
      });
      expect(indexes).toEqual([...indexes].sort((a, b) => a - b));
    });

    it("keeps the checks it always ran: install, typecheck, lint, the tests and the speech runtime", () => {
      const runs = windows().map((step) => step.run ?? "").join("\n");
      for (const command of ["npm ci", "npm run typecheck", "npm run lint", "npm test", "build-speech-runtime.ps1"]) expect(runs).toContain(command);
    });

    it("no longer packages with Electron or makes a block map", () => {
      const text = JSON.stringify(jobs.windows);
      for (const word of ["npm run package", "test:packaged", "electron-builder", "blockmap"]) expect(text, word).not.toContain(word);
    });

    it("checks the production binaries (N3) and the crate versions, and is lenient on versions only in a dry run", () => {
      const binaries = findStep("windows", /check-production-binaries\.ps1/).run ?? "";
      expect(binaries).toContain("-Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe");
      expect(binaries).toContain("-Daemon tjommie-rs\\target\\release\\tjommie-daemon.exe");
      const crates = findStep("windows", /check-release-version\.ts/).run ?? "";
      expect(crates).toContain("if ($env:DRY_RUN -eq 'true') { $strict += '--warn' }");
      expect(crates).toContain("$strict = @()");
      expect(crates).toContain("$LASTEXITCODE -ne 0");
    });

    it("assembles the payload for production, then builds, hashes and checks the installer", () => {
      const build = findStep("windows", /\.\/installer\/build-installer\.ps1/).run ?? "";
      expect(build).toContain("-Identity production");
      expect(build).toContain("-Version $env:VERSION");
      expect(build).toContain('Tjommie-Setup-$env:VERSION.exe');
      const latest = findStep("windows", /make-latest-yml\.ts/).run ?? "";
      expect(latest).toContain("--installer");
      expect(latest).toContain("--out dist-desktop/installer/latest.yml");
      const check = findStep("windows", /check-release-installer\.ps1/).run ?? "";
      expect(check).toContain("-LatestYml dist-desktop\\installer\\latest.yml");
      expect(check).toContain("-Version $env:VERSION");
    });

    it("uploads the release's files under windows-assets only for a release, and the dry run's under its own name", () => {
      const release = windows().filter((step) => step.with?.name === "windows-assets");
      const dry = windows().filter((step) => step.with?.name === "release-dryrun-windows");
      expect(release).toHaveLength(1);
      expect(dry).toHaveLength(1);
      expect(release[0].if).toBe("${{ needs.verify.outputs.dry_run == 'false' }}");
      expect(dry[0].if).toBe("${{ needs.verify.outputs.dry_run == 'true' }}");
      expect(String(release[0].with?.path)).not.toContain("release-manifest.json");
      for (const expected of ["Tjommie-Setup-*.exe", "latest.yml", "release-manifest.json"]) expect(String(dry[0].with?.path)).toContain(expected);
      expect(dry[0].with?.["if-no-files-found"]).toBe("error");
    });

    it("fails before it builds anything if the installer's files are not in this checkout", () => {
      const early = findStep("windows", /installer's files are here/).run ?? "";
      for (const name of ["tjommie.nsi", "build-installer.ps1", "identities.json", "helper-log.ps1", "scoped-processes.ps1", "stop-install-processes.ps1", "unlink-reparse-points.ps1", "set-shortcut-aumid.ps1"]) expect(early).toContain(name);
      expect(early).toContain("throw");
      expect(windows().findIndex((step) => /installer's files are here/.test(step.name ?? ""))).toBeLessThan(windows().findIndex((step) => step.run === "npm ci"));
    });
  });

  describe("the other jobs", () => {
    it("the verify job keeps the tag, main and CHANGELOG checks, but only for a release", () => {
      const check = findStep("verify", /Check tag and extract release notes/);
      expect(check.if).toBe("${{ steps.mode.outputs.dry_run == 'false' }}");
      for (const kept of ['"$GITHUB_REF_NAME" != "v$version"', "git merge-base --is-ancestor", "apply-native.mjs --check", "changelog-section.ts"]) expect(check.run).toContain(kept);
      const notes = stepsOf("verify").find((step) => step.with?.name === "release-notes");
      expect(notes?.if).toBe("${{ steps.mode.outputs.dry_run == 'false' }}");
    });

    it("a dry run only reports the Android version map, and does not fail on it", () => {
      const report = findStep("verify", /Check the Android version map/);
      expect(report["continue-on-error"]).toBe(true);
      expect(report.if).toBe("${{ steps.mode.outputs.dry_run == 'true' }}");
      expect(report.run).toContain("apply-native.mjs --check");
    });

    it("the Android build is a release-only job", () => {
      expect(jobs.android.if).toBe("${{ needs.verify.outputs.dry_run == 'false' }}");
      expect(needsOf(jobs.android)).toEqual(["verify"]);
    });
  });

  it("calls only scripts that exist here or come from the installer work (T8.2)", () => {
    const FROM_T82 = ["installer/build-installer.ps1", "installer/tjommie.nsi"];
    const referenced = new Set<string>();
    for (const { step } of everyStep()) {
      for (const match of (step.run ?? "").matchAll(/\b((?:scripts|installer|packages)[\\/][\w.\\/-]+\.(?:ts|mjs|ps1|nsi))\b/g)) referenced.add(match[1].replaceAll("\\", "/"));
    }
    expect([...referenced].length).toBeGreaterThan(8);
    const missing = [...referenced].filter((path) => !existsSync(join(repo, path)));
    for (const path of missing) expect(FROM_T82, `${path} is neither here nor from the installer work`).toContain(path);
  });

  describe("every run block parses", () => {
    const scripts = (shell: string): { name: string; script: string }[] =>
      everyStep().flatMap(({ job, step }) => {
        if (!step.run) return [];
        const effective = step.shell ?? ((jobs[job]["runs-on"] ?? "").includes("windows") ? "pwsh" : "bash");
        const name = `${job}-${(step.name ?? step.run.split("\n")[0]).replace(/[^\w]+/g, "-").slice(0, 50)}`;
        return effective === shell ? [{ name, script: step.run.replace(/\$\{\{[\s\S]*?\}\}/g, "X") }] : [];
      });
    const powershell = ((): string => {
      for (const candidate of ["pwsh", "powershell"]) if (spawnSync(candidate, ["-NoProfile", "-Command", "exit 0"], { windowsHide: true }).status === 0) return candidate;
      return "";
    })();
    const hasBash = spawnSync("bash", ["--version"], { windowsHide: true }).status === 0;

    it.skipIf(!powershell)("the PowerShell steps are valid PowerShell", () => {
      const folder = mkdtempSync(join(tmpdir(), "tjommie-workflow-test-"));
      folders.push(folder);
      const found = scripts("pwsh");
      expect(found.length).toBeGreaterThan(10);
      mkdirSync(join(folder, "steps"));
      for (const { name, script } of found) writeFileSync(join(folder, "steps", `${name}.ps1`), script);
      writeFileSync(
        join(folder, "check.ps1"),
        [
          "param([string]$Folder)",
          "$bad = 0",
          "foreach ($file in Get-ChildItem -LiteralPath $Folder -Filter *.ps1) {",
          "  $tokens = $null; $errors = $null",
          "  [void][System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)",
          '  foreach ($e in $errors) { "$($file.Name): line $($e.Extent.StartLineNumber): $($e.Message)"; $bad++ }',
          "}",
          "if ($bad -gt 0) { exit 1 }",
          "exit 0",
        ].join("\n"),
      );
      const result = spawnSync(powershell, ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(folder, "check.ps1"), "-Folder", join(folder, "steps")], { encoding: "utf8", windowsHide: true });
      expect(result.status, `${result.stdout}${result.stderr}`).toBe(0);
    });

    it.skipIf(!hasBash)("the bash steps are valid bash", () => {
      const found = scripts("bash");
      expect(found.length).toBeGreaterThan(1);
      for (const { name, script } of found) {
        const result = spawnSync("bash", ["-n"], { input: script, encoding: "utf8", windowsHide: true });
        expect(result.status, `${name}: ${result.stderr}`).toBe(0);
      }
    });
  });
});
