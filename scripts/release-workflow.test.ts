import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { releaseMode } from "./release-mode.ts";
import { mutations } from "./release-workflow-mutations.ts";

// T8.3: the release workflow has a dry run that cannot publish. GitHub only runs a workflow on a runner, so this file is how it is proved
// here: every workflow is parsed with a real YAML parser (GitHub refuses a bad one before any job starts, with no log), and release.yml is
// then walked as GitHub would walk it, for each kind of event, to show that outside a push of a v* tag no step creates a release, pushes a
// tag or touches a latest.yml, and that the one job allowed to do so is skipped.
//
// What actually keeps a dry run from publishing is the permissions model: `contents: read` for the workflow, `contents: write` for the
// release job alone, no checkout that keeps its credentials, no token outside that job, and the release job's own `if`. Those are pinned
// exactly (the permission, token and condition tests below). The lists of commands that must not appear (PUBLISHING, and the one for
// script files) are a REGRESSION NET, not a proof: text can always be spelled in a way a list does not know. They catch the obvious change
// and the ones the review of this file named; they are not what the safety rests on.
//
// scripts/release-workflow-mutations.ts holds the deliberate breakages this file is checked against, and runs them
// (`node scripts/release-workflow-mutations.ts`); a test here fails if one of them has gone stale.

vi.setConfig({ testTimeout: 60_000 });

const repo = resolve(import.meta.dirname, "..");
const workflowDir = join(repo, ".github", "workflows");
const parser = join(import.meta.dirname, "parse-yaml.mjs");
// The mutation runner points this at a mutated copy of release.yml; the repository's own file is never touched.
const releaseFile = process.env.TJOMMIE_RELEASE_WORKFLOW ?? join(workflowDir, "release.yml");

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

// What a script can run, as opposed to what it says. The release test follows the scripts a workflow step runs, and the scripts those
// name, so it must not follow a name that only appears in a comment. This removes the comments and leaves strings alone (a `;` or `#` or
// `//` inside a string is not a comment). It is a small scanner, not a parser, and goes as far as these scripts do: line comments,
// PowerShell <# #> and NSIS/JavaScript /* */ blocks, quoted strings, PowerShell here-strings. Known limit: a JavaScript regex literal that
// ends in `//` is taken for a comment, and the rest of its line is lost.
type Syntax = "powershell" | "bash" | "nsis" | "script";
const syntaxOfFile = (path: string): Syntax => (/\.ps1$/i.test(path) ? "powershell" : /\.(?:nsi|nsh)$/i.test(path) ? "nsis" : "script");

function withoutComments(text: string, syntax: Syntax): string {
  const shell = syntax === "powershell" || syntax === "bash";
  // The index after the string that opens at `from`, by this syntax's own escapes.
  const stringEnd = (from: number, quote: string): number => {
    for (let j = from + 1; j < text.length; j++) {
      const c = text[j];
      if (syntax === "nsis") {
        if (c === "$" && text[j + 1] === "\\") { j++; continue; }
        if (c === "\n") return j; // an NSIS string does not run over a line
      } else if (syntax === "powershell") {
        if (c === "`" && quote === '"') { j++; continue; }
        if (c === quote && text[j + 1] === quote) { j++; continue; } // '' and "" are a quote inside
      } else if (syntax === "bash") {
        if (c === "\\" && quote === '"') { j++; continue; }
      } else {
        if (c === "\\") { j++; continue; }
        if (c === "\n" && quote !== "`") return j;
      }
      if (c === quote) return j + 1;
    }
    return text.length;
  };
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    const two = text.slice(i, i + 2);
    if (syntax === "powershell" && two === "<#") {
      const end = text.indexOf("#>", i + 2);
      out += " ";
      i = end === -1 ? text.length : end + 2;
      continue;
    }
    if ((syntax === "nsis" || syntax === "script") && two === "/*") {
      const end = text.indexOf("*/", i + 2);
      out += " ";
      i = end === -1 ? text.length : end + 2;
      continue;
    }
    // A shell or NSIS comment starts at the start of a word, so `a#b` and `"x;y"` are not one.
    const startsWord = i === 0 || /[\s;(){},|&=]/.test(text[i - 1]);
    const lineComment = shell ? c === "#" && startsWord : syntax === "nsis" ? (c === ";" || c === "#") && startsWord : two === "//";
    if (lineComment) {
      while (i < text.length && text[i] !== "\n") i++;
      continue;
    }
    if (syntax === "powershell" && c === "@" && (text[i + 1] === "'" || text[i + 1] === '"') && /^[ \t]*\r?\n/.test(text.slice(i + 2, i + 202))) {
      const terminator = `\n${text[i + 1]}@`; // a here-string ends on a line that starts with '@ or "@
      const end = text.indexOf(terminator, i + 2);
      const stop = end === -1 ? text.length : end + terminator.length;
      out += text.slice(i, stop);
      i = stop;
      continue;
    }
    if (c === "'" || c === '"' || (c === "`" && (syntax === "nsis" || syntax === "script"))) {
      const end = stringEnd(i, c);
      out += text.slice(i, end);
      i = end;
      continue;
    }
    if (syntax === "bash" && c === "\\") {
      out += text.slice(i, i + 2); // an escaped character, which may be a # or a quote
      i += 2;
      continue;
    }
    out += c;
    i++;
  }
  return out;
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
  const workflow = parseWorkflow(releaseFile);
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
  // The repository scripts a run: block names, and the scripts those name in turn, and so on until nothing new is named (to a fixed point,
  // not one level), so a step that runs a script is judged by every script it can reach. "Name" is read from CODE: each script has its
  // comments removed first (withoutComments), because a name in a comment is not a call (dry run 2: hosted-runner.ps1 says in a comment
  // that install-latest-r1.ps1 is something else, and that script uses `gh release` and is run by another workflow). What is named is a
  // path in the code (scripts/x.ts, installer/y.ps1), or a file of the same folder by name (Join-Path $PSScriptRoot 'x.ps1', an NSIS
  // `File "...\x.ps1"`). A name inside a string that is not a call is followed too, which only widens what is checked. An `npm run` is
  // not followed.
  const SCRIPT_PATH = /\b((?:scripts|installer|packages)[\\/][\w.\\/-]+\.(?:ts|mjs|ps1|nsi))\b/g;
  const named = (code: string): string[] => [...code.matchAll(SCRIPT_PATH)].map((match) => match[1].replaceAll("\\", "/"));
  const namesFile = (code: string, name: string): boolean => new RegExp(`(?<![\\w.-])${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![\\w-])`, "i").test(code);
  const scriptsCalledBy = (text: string, syntax: Syntax, root = repo): string[] => {
    const found = new Set<string>();
    const queue = named(withoutComments(text, syntax));
    for (let path = queue.pop(); path !== undefined; path = queue.pop()) {
      if (found.has(path) || !existsSync(join(root, path))) continue;
      found.add(path);
      const code = withoutComments(readFileSync(join(root, path), "utf8"), syntaxOfFile(path));
      queue.push(...named(code));
      const folder = path.slice(0, path.lastIndexOf("/") + 1);
      for (const sibling of readdirSync(join(root, folder))) {
        if (/\.(?:ts|mjs|ps1|nsi|nsh)$/.test(sibling) && `${folder}${sibling}` !== path && namesFile(code, sibling)) queue.push(`${folder}${sibling}`);
      }
    }
    return [...found];
  };
  // The syntax of a step's own text: the shell it runs under (pwsh on a Windows runner unless it says otherwise, bash elsewhere).
  const syntaxOfStep = (job: string, step: Step): Syntax => {
    const shell = step.shell ?? ((jobs[job]["runs-on"] ?? "").includes("windows") ? "pwsh" : "bash");
    return shell === "pwsh" || shell === "powershell" ? "powershell" : "bash";
  };

  // Anything in a step's own text that creates a release, moves a tag or code, calls the GitHub API or posts to a service. Only the
  // release job may have these. A regression net (see the header), widened by the review's F3.
  const PUBLISHING: RegExp[] = [
    // The GitHub CLI in any spelling: gh, gh.exe, 'gh', "gh", & gh.exe, Start-Process -FilePath gh.exe. Not a word that merely holds "gh".
    /(?<![\w.-])['"]?gh(?:\.exe)?['"]?(?=\s|$)/im,
    // git with options before the subcommand (git -c user.name=x push) as well as directly. merge-base only reads history (the verify job
    // asks whether the tag is on main), so "merge" must not be followed by "-base".
    /\bgit(?:\.exe)?['"]?\b[^\n;|&]*?\s(?:push|tag|commit|remote|config|am|cherry-pick|update-ref|rebase|reset|merge(?![\w-]))\b/i,
    /--verify-tag/,
    // Calling out over HTTP from a step: command-line clients, the PowerShell cmdlets, .NET clients, BITS, and code built from text.
    /\b(?:curl|wget|Invoke-WebRequest|Invoke-RestMethod|iwr|irm|Start-BitsTransfer|Invoke-Command|Invoke-Expression|iex)\b/i,
    /\bNet\.(?:WebClient|Http\.HttpClient|WebRequest|Sockets)\b|\b(?:WebClient|HttpClient|HttpWebRequest)\b/i,
    // Code run from the command line instead of from a file: node -e, and any inline script that fetches or opens an HTTP connection.
    // (node -p "require('./package.json').version", which only reads a version, is fine.)
    /\bnode(?:\.exe)?\s+(?:-e|--eval)\b/i,
    /\bfetch\s*\(|\brequire\(\s*['"](?:node:)?https?['"]\s*\)|\bimport\s*\(\s*['"](?:node:)?https?['"]/i,
    /\bnpx\b|\boctokit\b/i,
    /\b(?:npm|cargo|yarn|pnpm)\s+publish\b/,
    /\bwrangler\b/i,
  ];
  // The same, for a script file a step runs, where a download is ordinary (the speech runtime is fetched): only what reaches GitHub's
  // write side, or a credential, counts there.
  const PUBLISHING_IN_SCRIPTS: RegExp[] = [
    /(?<![\w.-])['"]?gh(?:\.exe)?['"]?\s+(?:release|api|workflow|run|pr|issue|repo|secret|auth|gist)\b/i,
    /\bapi\.github\.com\b|\buploads\.github\.com\b/i,
    /\bgit(?:\.exe)?['"]?\b[^\n;|&]*?\s(?:push|tag|update-ref|merge(?![\w-]))\b/i,
    /--verify-tag|\bGITHUB_TOKEN\b|\bGH_TOKEN\b|\bsecrets\./i,
    /\boctokit\b|\bcreateRelease\b|\bcreate_release\b/i,
  ];
  const publishingIn = (text: string, patterns: RegExp[] = PUBLISHING): RegExp[] => patterns.filter((pattern) => pattern.test(text));
  // The scripts a step's text can reach that reach GitHub's write side. The scripts are read as they are, comments and all: a comment that
  // says `gh release create` fails here too, which is the safe way round (only which scripts are followed ignores comments).
  const publishingScripts = (text: string, syntax: Syntax, root = repo): string[] =>
    scriptsCalledBy(text, syntax, root).filter((path) => publishingIn(readFileSync(join(root, path), "utf8"), PUBLISHING_IN_SCRIPTS).length > 0);
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

    // The owner's F2 decision (2026-11-15): no 0.5.x release after 0.6.0. Before anything is created the latest release's version is read, and
    // this tag has to be strictly greater. The step's own text is what runs here, against a stand-in `gh`, so what is tried is what ships.
    describe("the release must be newer than the latest one", () => {
      const name = "The release is newer than the latest one";
      const guard = (): string => findStep("release", /The release is newer than the latest one/).run ?? "";

      it("is a step of the publish job, ahead of the step that creates the release, and only reads", () => {
        const names = stepsOf("release").map((step) => step.name ?? "");
        expect(names.indexOf(name), "the guard is there").toBeGreaterThanOrEqual(0);
        expect(names.indexOf(name), "and comes first").toBeLessThan(names.indexOf("Publish GitHub release"));
        expect(findStep("release", /The release is newer than the latest one/).shell).toBe("bash");
        const script = guard();
        expect(script).toContain("gh release view --json tagName");
        expect(script).not.toMatch(/gh release (?:create|edit|upload|delete)|--latest|\bgit\s+(?:push|tag)\b/);
        for (const other of jobNames) if (other !== "release") expect(stepsOf(other).some((step) => step.name === name), `${other} has no such step`).toBe(false);
      });

      type Stub = { out?: string; err?: string; code?: number };
      // Runs the guard as GitHub runs a bash step (bash -eo pipefail) with `gh` replaced by a function that answers only `gh release view
      // --json tagName ...` and fails any other call, so a guard that did something else would show.
      const runGuard = (tag: string, stub: Stub): { status: number | null; output: string } => {
        const dir = mkdtempSync(join(tmpdir(), "tjommie-guard-test-"));
        folders.push(dir);
        writeFileSync(join(dir, "guard.sh"), guard());
        writeFileSync(
          join(dir, "run.sh"),
          [
            "gh() {",
            '  if [[ "$1" == release && "$2" == view && "$3" == --json && "$4" == tagName ]]; then',
            '    [[ -n "$STUB_ERR" ]] && echo "$STUB_ERR" >&2',
            '    [[ -n "$STUB_OUT" ]] && echo "$STUB_OUT"',
            '    return "${STUB_CODE:-0}"',
            "  fi",
            '  echo "unexpected gh call: $*" >&2',
            "  return 99",
            "}",
            "source ./guard.sh",
          ].join("\n"),
        );
        const run = spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "run.sh"], {
          cwd: dir,
          encoding: "utf8",
          windowsHide: true,
          env: { ...process.env, GITHUB_REF_NAME: tag, STUB_OUT: stub.out ?? "", STUB_ERR: stub.err ?? "", STUB_CODE: String(stub.code ?? 0) },
        });
        return { status: run.status, output: `${run.stdout}${run.stderr}` };
      };
      const bashAvailable = spawnSync("bash", ["--version"], { windowsHide: true }).status === 0;

      describe.skipIf(!bashAvailable)("against a stand-in gh (these need bash)", () => {
      it.each([
        ["the real case: 0.6.0 after 0.5.1", "v0.6.0", "v0.5.1"],
        ["a newer patch", "v0.6.1", "v0.6.0"],
        ["a newer minor over a larger patch", "v0.7.0", "v0.6.9"],
        ["a newer major", "v1.0.0", "v0.99.99"],
        ["numbers, not text: 0.10.0 is newer than 0.9.0", "v0.10.0", "v0.9.0"],
        ["a release over a prerelease of the same version", "v0.6.0", "v0.6.0-rc.1"],
        ["a release over a prerelease with build metadata", "v0.6.0", "v0.6.0-rc.1+build.5"],
        ["a latest tagged without the v", "v0.6.0", "0.5.1"],
      ])("lets %s through", (_label, tag, latest) => {
        const run = runGuard(tag, { out: latest });
        expect(run.status, run.output).toBe(0);
        expect(run.output).toContain("is newer than the latest release");
      });

      it.each([
        ["an older version (a 0.5.x after 0.6.0)", "v0.5.2", "v0.6.0", /older than the latest release, v0\.6\.0/],
        ["an older version, by number and not by text", "v0.9.0", "v0.10.0", /older than the latest release/],
        ["an older major", "v0.9.9", "v1.0.0", /older than the latest release/],
        ["the same version", "v0.6.0", "v0.6.0", /same version as the latest release/],
        ["the same version, the latest with build metadata", "v0.6.0", "v0.6.0+abc", /same version as the latest release/],
        ["a prerelease of a later version being the latest", "v0.6.0", "v0.6.1-rc.1", /older than the latest release/],
      ])("refuses %s, and says why", (_label, tag, latest, message) => {
        const run = runGuard(tag, { out: latest });
        expect(run.status, run.output).toBe(1);
        expect(run.output).toMatch(message);
      });

      it.each(["latest", "v0.6","release-2026-11", "v1.2.3.4", "v01.2.3", "v0.6.0-", "v0.6.0-rc..1", "v0.6.99999999999", "v0.6.0\nv0.5.0", ""])(
        "refuses a latest release it cannot compare with: %j",
        (latest) => {
          const run = runGuard("v0.7.0", { out: latest });
          expect(run.status, run.output).toBe(1);
          expect(run.output).toContain("not a semantic version");
        },
      );

      it.each(["v0.6", "v0.6.0-rc.1","v00.6.0", "vX", "v0.6.0.1", "v"])("refuses a tag that is not v<major>.<minor>.<patch>: %s", (tag) => {
        const run = runGuard(tag, { out: "v0.1.0" });
        expect(run.status, run.output).toBe(1);
        expect(run.output).toContain("is not v<major>.<minor>.<patch>");
      });

      it("lets the first release through, and refuses when the latest cannot be read at all", () => {
        const first = runGuard("v0.6.0", { err: "release not found", code: 1 });
        expect(first.status, first.output).toBe(0);
        expect(first.output).toContain("is the first");
        for (const err of ["HTTP 502: Bad Gateway", "gh: To use GitHub CLI in a GitHub Actions workflow, set the GH_TOKEN environment variable.", "could not resolve host"]) {
          const run = runGuard("v0.6.0", { err, code: 1 });
          expect(run.status, `${err}: ${run.output}`).toBe(1);
          expect(run.output).toContain("could not be read");
          expect(run.output).toContain(err);
        }
      });
      });
    });

    it("publishes the installer, latest.yml and the APK, with no block map", () => {
      const script = findStep("release", /Publish GitHub release/).run ?? "";
      for (const asset of ['"release-assets/Tjommie-Setup-$version.exe"', "release-assets/latest.yml", '"release-assets/Tjommie-Android-$version.apk"']) expect(script).toContain(asset);
      expect(script).not.toMatch(/blockmap/i);
    });

    it("only downloads artifacts, apart from the version guard and the publishing step", () => {
      const others = stepsOf("release").filter((step) => !/Publish GitHub release|The release is newer than the latest one/.test(step.name ?? ""));
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

    // An allowlist, not a search for one name: a token under any other name (`GH_TOKEN: ${{ secrets.RELEASE_PAT }}` on the Windows job) is
    // as good as the workflow's own, so a secret may be named only where it is expected, and a secret-looking variable only there too.
    it("keeps the workflow token and every secret where they belong, whatever they are called", () => {
      const androidSecrets = ["ANDROID_KEYSTORE_BASE64", "GOOGLE_SERVICES_JSON_BASE64", "ANDROID_KEYSTORE_PASSWORD", "ANDROID_KEY_ALIAS", "ANDROID_KEY_PASSWORD"];
      const androidSecretVariables = ["TJOMMIE_ANDROID_KEYSTORE_PASSWORD", "TJOMMIE_ANDROID_KEY_PASSWORD"];
      const secretish = /TOKEN|SECRET|PASSWORD|PASSPHRASE|(?:^|_)PAT(?:$|_)|API_?KEY|PRIVATE_?KEY|CREDENTIAL/i;
      const variablesOf = (job: Job): string[] => [...Object.keys(job.env ?? {}), ...(job.steps ?? []).flatMap((step) => Object.keys(step.env ?? {}))];
      for (const name of jobNames) {
        const text = JSON.stringify(jobs[name]);
        const secrets = [...text.matchAll(/\bsecrets\.([A-Za-z0-9_]+)/g)].map((match) => match[1]);
        const token = /\bgithub\.token\b|\bGITHUB_TOKEN\b|\bGH_TOKEN\b/.test(text);
        const secretLooking = variablesOf(jobs[name]).filter((variable) => secretish.test(variable));
        if (name === "release") {
          expect(secrets, "release names no secret of its own").toEqual([]);
          expect(token, "release holds the workflow token").toBe(true);
        } else if (name === "android") {
          for (const secret of secrets) expect(androidSecrets, `android may use secrets.${secret}`).toContain(secret);
          expect(token, "android has no workflow token").toBe(false);
          for (const variable of secretLooking) expect(androidSecretVariables, `android may set ${variable}`).toContain(variable);
        } else {
          expect(secrets, `${name} uses no secret`).toEqual([]);
          expect(token, `${name} has no token`).toBe(false);
          expect(secretLooking, `${name} sets no secret-looking variable`).toEqual([]);
        }
      }
      // The ways to reach a secret without naming it.
      expect(JSON.stringify(workflow)).not.toMatch(/\bsecrets\s*\[|toJSON\(\s*secrets|secrets:\s*inherit/);
      for (const { step } of everyStep()) if (step.uses?.startsWith("actions/checkout@")) expect(step.with?.["persist-credentials"]).toBe(false);
    });

    // The review's F3(2): a step that runs a repository script is judged by every script it can reach, by what they do as well as by its
    // own text. Followed to a fixed point, by what is named in code and not in comments (see scriptsCalledBy); an `npm run` is not followed.
    it("has no script, run from outside the publish job, that reaches GitHub's write side or a credential", () => {
      const called = new Set<string>();
      for (const { job, step } of everyStep()) {
        if (job === "release") continue;
        const syntax = syntaxOfStep(job, step);
        for (const path of scriptsCalledBy(step.run ?? "", syntax)) called.add(path);
        expect(publishingScripts(step.run ?? "", syntax), `${job}: "${step.name ?? step.run}" can reach a script that publishes`).toEqual([]);
      }
      expect(called.size, "it found the scripts the steps run").toBeGreaterThan(8);
    });

    describe("following the scripts a step runs", () => {
      const publishes = "gh release create v1 --notes x\n";
      const fixture = (files: Record<string, string>): string => {
        const root = mkdtempSync(join(tmpdir(), "tjommie-follow-test-"));
        folders.push(root);
        for (const [name, content] of Object.entries(files)) {
          mkdirSync(join(root, name, ".."), { recursive: true });
          writeFileSync(join(root, name), content);
        }
        return root;
      };

      it("takes comments out of each kind of script and leaves what is in a string", () => {
        const text = withoutComments;
        // PowerShell
        expect(text("Write-Output 'a # b' # gone\nnext", "powershell")).toBe("Write-Output 'a # b' \nnext");
        const quoted = text('Write-Output "a # b `" # still a string" # gone', "powershell");
        expect(quoted).toContain("still a string");
        expect(quoted).not.toContain("gone");
        expect(text("a <# gone\nstill gone #> b", "powershell")).not.toContain("gone");
        const here = text("$s = @'\n# kept\n'@\n# gone\n", "powershell");
        expect(here).toContain("# kept");
        expect(here).not.toContain("gone");
        expect(text("$a = 'it''s # kept' # gone", "powershell")).toContain("it''s # kept");
        // bash
        expect(text('echo "a # b" # gone', "bash")).toBe('echo "a # b" ');
        expect(text("echo a\\#b # gone", "bash")).toBe("echo a\\#b ");
        // NSIS
        expect(text('File "a;b.ps1" ; gone\n!define X "y" # gone\n/* gone */ Nop', "nsis")).toBe('File "a;b.ps1" \n!define X "y" \n  Nop');
        // TypeScript and JavaScript
        expect(text('const a = "http://x"; // gone\n/* gone */ x = `a//b`;', "script")).toBe('const a = "http://x"; \n  x = `a//b`;');
      });

      // Dry run 2: hosted-runner.ps1 names install-latest-r1.ps1 in a comment, that script uses `gh release`, and the release workflow never runs it.
      it("does not follow a script that is only named in a comment, and does follow a real call two levels down and catches it there", () => {
        const root = fixture({
          "installer/a.ps1": "# the other one, c.ps1, is not run from here\n. (Join-Path $PSScriptRoot 'b.ps1')\n<# nor e.ps1 #>\n",
          "installer/b.ps1": "# see c.ps1\n& (Join-Path $PSScriptRoot 'd.ps1')\n",
          "installer/c.ps1": publishes,
          "installer/d.ps1": publishes,
          "installer/e.ps1": publishes,
        });
        expect(scriptsCalledBy("./installer/a.ps1", "powershell", root).sort()).toEqual(["installer/a.ps1", "installer/b.ps1", "installer/d.ps1"]);
        expect(publishingScripts("./installer/a.ps1", "powershell", root)).toEqual(["installer/d.ps1"]); // two levels down, a real call: caught
        // A step's own comment does not name a script either.
        expect(scriptsCalledBy("# ./installer/c.ps1\n./installer/a.ps1", "powershell", root)).not.toContain("installer/c.ps1");
        expect(publishingScripts("./installer/c.ps1", "powershell", root)).toEqual(["installer/c.ps1"]); // but a step that does run it is caught
      });

      it("follows what an NSIS script includes, through the script that builds it, and not what it only mentions", () => {
        const root = fixture({
          "installer/build.ps1": "# Compiles installer/e.nsi, and nothing about h.ps1\n$nsi = Join-Path $PSScriptRoot 'e.nsi'\n",
          "installer/e.nsi": '; File "${__FILEDIR__}\\g.ps1"\n  File "${__FILEDIR__}\\f.ps1"\n',
          "installer/f.ps1": "Write-Output 'a helper'\n",
          "installer/g.ps1": publishes,
          "installer/h.ps1": publishes,
        });
        expect(scriptsCalledBy("./installer/build.ps1", "powershell", root).sort()).toEqual(["installer/build.ps1", "installer/e.nsi", "installer/f.ps1"]);
        expect(publishingScripts("./installer/build.ps1", "powershell", root)).toEqual([]);
      });

      // The same, on the real scripts: the helpers the installer really `File`s are reached, and the one that is only mentioned in a comment is not.
      it.skipIf(!existsSync(join(repo, "installer", "tjommie.nsi")))("reaches the helpers the real installer includes, and not install-latest-r1.ps1", () => {
        const called = new Set<string>();
        for (const { job, step } of everyStep()) if (job !== "release") for (const path of scriptsCalledBy(step.run ?? "", syntaxOfStep(job, step))) called.add(path);
        for (const helper of ["tjommie.nsi", "helper-log.ps1", "scoped-processes.ps1", "stop-install-processes.ps1", "unlink-reparse-points.ps1", "set-shortcut-aumid.ps1"]) {
          expect([...called], helper).toContain(`installer/${helper}`);
        }
        expect([...called], "named only in comments").not.toContain("installer/install-latest-r1.ps1");
      });
    });

    // What the review named, and what must stay allowed: the list is the net, and the net has to catch what it was widened for.
    it("catches the spellings the review named, and leaves ordinary commands alone", () => {
      const caught = [
        "gh release create v1 --notes x",
        "gh.exe release create v1",
        "& 'gh' release create v1",
        '& "gh" release create v1',
        "Start-Process -FilePath gh.exe -ArgumentList release",
        "git push origin HEAD",
        "git -c user.name=x push origin HEAD",
        "git.exe tag v9",
        "git commit -m x",
        "[Net.WebClient]::new().DownloadString('https://example.test')",
        "[System.Net.Http.HttpClient]::new()",
        "Start-BitsTransfer -Source https://example.test -Destination x",
        `node -e "fetch('https://api.github.com/repos/x/releases', { method: 'POST' })"`,
        "node --eval 'process.exit(0)'",
        "npx octokit rest",
        "Invoke-RestMethod -Method Post -Uri https://example.test",
        "iwr https://example.test",
        "curl -X POST https://example.test",
        "Invoke-Expression $payload",
        "gh release create v1 --verify-tag",
        "cargo publish -p x",
      ];
      for (const text of caught) expect(publishingIn(text).length, text).toBeGreaterThan(0);
      const allowed = [
        `node -p "require('./package.json').version"`,
        `git merge-base --is-ancestor "$GITHUB_SHA" origin/main`,
        "node scripts/changelog-section.ts 0.6.0 > notes.md",
        "cargo build --release --locked -p daemon -p desktop",
        "npm ci",
        "choco install nsis --version=3.11 --no-progress -y",
        "echo through the high ghost",
        "Get-ChildItem -Recurse",
      ];
      for (const text of allowed) expect(publishingIn(text).map(String), text).toEqual([]);
      const scriptsCaught = ["gh release create v1", "Invoke-RestMethod https://api.github.com/repos/x/releases", "git push origin HEAD", "$env:GITHUB_TOKEN", "--verify-tag"];
      for (const text of scriptsCaught) expect(publishingIn(text, PUBLISHING_IN_SCRIPTS).length, text).toBeGreaterThan(0);
      const scriptsAllowed = ["Invoke-WebRequest https://example.test/model.bin -OutFile model.bin", "git clone https://example.test/repo.git", "git -C $source rev-parse HEAD", "git merge-base --is-ancestor a b"];
      for (const text of scriptsAllowed) expect(publishingIn(text, PUBLISHING_IN_SCRIPTS).map(String), text).toEqual([]);
    });
  });

  // The mutations scripts/release-workflow-mutations.ts runs against this file. Each replaces one piece of release.yml, so each has to be
  // found exactly once: a mutation that matches nothing tests nothing, and "caught" would then mean nothing.
  it("has a mutation script that has not gone stale", () => {
    // LF, as the mutations are written: a Windows checkout may have CRLF.
    const text = readFileSync(join(workflowDir, "release.yml"), "utf8").replaceAll("\r\n", "\n");
    expect(mutations.length).toBeGreaterThanOrEqual(42);
    for (const { label, from } of mutations) expect(text.split(from).length - 1, `"${label}": its text is found once in release.yml`).toBe(1);
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

    it.each(dryRuns)("%s builds and checks, publishes nothing, and uploads only the dry run's own artifacts", (_label, event, ref) => {
      const context = contextFor(event, ref);
      expect(context.dryRun).toBe("true");
      const run = walk(context);
      expect(run.jobs).toEqual(["verify", "windows"]);
      expect(uploads(run.steps)).toEqual(["release-dryrun-tool-hashes", "release-dryrun-windows"]);
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

    // The provider-fixtures review's F2: the daemon is built under the production identity too, which is what makes the fixtures' compile_error!
    // fire if they were ever switched on. It is one cargo invocation for both crates, so one environment covers both; nothing else builds a
    // crate of this repository in a job of this workflow (a second, bare `cargo build -p daemon` would have no identity).
    it("builds the daemon only in the invocation that sets the production identity, and builds nothing anywhere else", () => {
      const builders = everyStep().filter(({ step }) => /\bcargo(?:\.exe)?\s+(?:build|install|run|rustc|test|check|clippy|doc)\b/.test(step.run ?? ""));
      expect(builders.map(({ job, step }) => `${job}: ${step.run}`)).toEqual(["windows: cargo build --release --locked -p daemon -p desktop"]);
      const [only] = builders;
      expect(only.step.env?.TJOMMIE_IDENTITY).toBe("production");
      expect(only.step.run).toContain("-p daemon");
      expect(only.step.run).toContain("-p desktop");
      // No identity is set anywhere but there: not at the job, so a step cannot quietly depend on it, and not to anything else.
      expect(JSON.stringify(jobs.windows.env ?? {})).not.toContain("IDENTITY");
    });

    // And the finished daemon is looked at, on the raw file, before NSIS: the production-binary check names the very files cargo wrote.
    it("checks the raw daemon cargo wrote, ahead of the installer, for the test driver and the provider fixtures", () => {
      const names = windows().map((step) => `${step.name ?? ""}\n${step.run ?? ""}`);
      const check = names.findIndex((text) => /check-production-binaries\.ps1/.test(text));
      const nsis = names.findIndex((text) => /choco install nsis/.test(text));
      const assemble = names.findIndex((text) => /assemble-payload\.ps1/.test(text));
      expect(check, "the check is there").toBeGreaterThanOrEqual(0);
      expect(check).toBeLessThan(assemble);
      expect(check).toBeLessThan(nsis);
      const step = findStep("windows", /check-production-binaries\.ps1/);
      expect(step.name).toBe("The production binaries have the update feed, no test driver and no provider fixtures");
      expect(step.run).toContain("-Daemon tjommie-rs\\target\\release\\tjommie-daemon.exe");
      expect(step.run).toContain("-Desktop tjommie-rs\\target\\release\\tjommie-desktop.exe");
    });

    it("runs its steps in the order that makes each check meaningful", () => {
      const order: RegExp[] = [
        /npm run typecheck/,
        /build-speech-runtime\.ps1/,
        /cargo build --release --locked/,
        /check-production-binaries\.ps1/,
        /check-release-version\.ts/,
        /fetch-pdfium\.ps1/,
        /assemble-payload\.ps1/,
        /choco install nsis --version=3\.11/,
        /check-tool-hashes\.ps1 -Tool nsis/,
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

    // The review's F1: NSIS is fetched by version only, so what Chocolatey installed is compared with recorded SHA-256 values before it compiles.
    it("checks NSIS against the recorded hashes before it compiles, and holds a release to them", () => {
      const run = findStep("windows", /check-tool-hashes\.ps1 -Tool nsis/).run ?? "";
      expect(run).toContain('-Tool nsis -Path "${env:ProgramFiles(x86)}\\NSIS"');
      expect(run).toContain("-Observed dist-desktop\\tool-hashes-nsis.json");
      // Anything but an exact 'true' is a release, as for the payload: a lost mode output takes the strict path, where a tool nobody pinned fails.
      expect(run).toContain("if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }");
      expect(run).toMatch(/check-tool-hashes\.ps1 [^\n]*@more/);
      expect(findStep("windows", /write-release-manifest\.ps1/).run).toContain('-ToolHashes "$root\\dist-desktop\\tool-hashes-nsis.json"');
    });

    // pdfium and its licence texts: fetched by the Chat lane's script into a folder of its own (the assembler never writes into a folder that has
    // anything in it), checked against the recorded hash, and handed to the assembler by name; a release without them is stopped twice.
    it("fetches pdfium into a folder of its own, checks it against the recorded hash, and hands the DLL and its licences to the assembler", () => {
      const fetch = findStep("windows", /fetch-pdfium\.ps1/);
      const run = fetch.run ?? "";
      expect(fetch.name).toBe("Fetch pdfium and check it against the recorded hash");
      expect(run).toContain("$dir = Join-Path $env:RUNNER_TEMP 'pdfium'");
      expect(run).not.toContain("payload");
      expect(run).toContain('& $fetch -Destination "$dir\\pdfium.dll" -Licenses "$dir\\licenses"');
      expect(run).toContain('./installer/check-tool-hashes.ps1 -Tool pdfium -Path "$dir\\pdfium.dll" @more');
      // As for NSIS: anything but an exact 'true' is a release, where the check and a missing script are both fatal.
      expect(run).toContain("if ($env:DRY_RUN -ne 'true') { $more['Release'] = $true }");
      expect(run).toContain("} elseif ($env:DRY_RUN -ne 'true') {");
      expect(run).toContain("throw ");
      const assemble = findStep("windows", /assemble-payload\.ps1/).run ?? "";
      expect(assemble).toContain("$pdfium = Join-Path $env:RUNNER_TEMP 'pdfium'");
      expect(assemble).toContain('$more[\'Pdfium\'] = "$pdfium\\pdfium.dll"');
      expect(assemble).toContain('$more[\'PdfiumLicenses\'] = "$pdfium\\licenses"');
      expect(assemble).toMatch(/assemble-payload\.ps1 [^\n]*@more/);
      expect(assemble).toContain("-OutDir \"$env:RUNNER_TEMP\\payload\"");
    });

    it("uploads the release's files under windows-assets only for a release, and the dry run's under its own name", () => {
      const release = windows().filter((step) => step.with?.name === "windows-assets");
      const dry = windows().filter((step) => step.with?.name === "release-dryrun-windows");
      expect(release).toHaveLength(1);
      expect(dry).toHaveLength(1);
      expect(release[0].if).toBe("${{ needs.verify.outputs.dry_run == 'false' }}");
      expect(dry[0].if).toBe("${{ needs.verify.outputs.dry_run == 'true' }}");
      expect(String(release[0].with?.path)).not.toContain("release-manifest.json");
      for (const expected of ["Tjommie-Setup-*.exe", "latest.yml", "release-manifest.json", "tool-hashes-nsis.json"]) expect(String(dry[0].with?.path)).toContain(expected);
      expect(String(release[0].with?.path)).not.toContain("tool-hashes");
      // What the NSIS check saw is kept even when the check fails (the always()), for a dry run only, under a name of its own.
      const seen = windows().filter((step) => step.with?.name === "release-dryrun-tool-hashes");
      expect(seen).toHaveLength(1);
      expect(seen[0].if).toBe("${{ always() && needs.verify.outputs.dry_run == 'true' }}");
      expect(seen[0].with).toMatchObject({ path: "dist-desktop/tool-hashes-nsis.json", "if-no-files-found": "ignore" });
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
