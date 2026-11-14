import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// GitHub rejects a workflow that is not valid YAML before any job starts, so nothing runs and there is no log to read (run
// 43641926017 was an unquoted ": " in a name). Every workflow file is parsed here with a real YAML parser (scripts/parse-yaml.mjs,
// js-yaml), and checked for the shape GitHub needs: a name, a trigger, and jobs that each have a runner and steps.

const repo = resolve(import.meta.dirname, "..");
const folder = join(repo, ".github", "workflows");
const parser = join(import.meta.dirname, "parse-yaml.mjs");

type Parsed = { status: number | null; json: unknown; error: string };

function parse(file: string): Parsed {
  const run = spawnSync(process.execPath, [parser, file], { encoding: "utf8", windowsHide: true });
  return { status: run.status, json: run.status === 0 ? (JSON.parse(run.stdout) as unknown) : null, error: run.stderr };
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

const workflows = readdirSync(folder).filter((name) => /\.ya?ml$/.test(name)).sort();
const scratch: string[] = [];
afterEach(() => { for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true }); });

describe("the GitHub workflows", () => {
  it("includes the ones this repository relies on", () => {
    expect(workflows).toEqual(expect.arrayContaining(["ci.yml", "release.yml", "desktop-acceptance.yml"]));
  });

  it.each(workflows)("%s is valid YAML with a name, a trigger and jobs that have a runner and steps", (name) => {
    const parsed = parse(join(folder, name));
    expect(parsed.status, `${name} is not valid YAML: ${parsed.error}`).toBe(0);
    const workflow = parsed.json;
    if (!isRecord(workflow)) throw new Error(`${name} is not a mapping.`);
    expect(typeof workflow.name, `${name}: name`).toBe("string");
    expect(workflow.on, `${name}: on`).toBeDefined();
    const jobs = workflow.jobs;
    if (!isRecord(jobs)) throw new Error(`${name}: jobs is not a mapping.`);
    expect(Object.keys(jobs).length, `${name}: jobs`).toBeGreaterThan(0);
    for (const [id, job] of Object.entries(jobs)) {
      if (!isRecord(job)) throw new Error(`${name}: job ${id} is not a mapping.`);
      // A job that calls a reusable workflow has `uses`; every other job has a runner and steps.
      if (job.uses !== undefined) continue;
      expect(job["runs-on"], `${name}: ${id} runs-on`).toBeDefined();
      const steps = job.steps;
      if (!Array.isArray(steps) || steps.length === 0) throw new Error(`${name}: job ${id} has no steps.`);
      steps.forEach((step: unknown, index) => {
        if (!isRecord(step)) throw new Error(`${name}: ${id} step ${index} is not a mapping.`);
        expect(["run", "uses"].filter((key) => step[key] !== undefined), `${name}: ${id} step ${index} needs run or uses`).toHaveLength(1);
      });
    }
  });

  // The check has teeth: the very line GitHub rejected is refused by the parser, and its quoted form is not.
  it("refuses an unquoted ': ' in a name, as GitHub does, and accepts it quoted", () => {
    const dir = mkdtempSync(join(tmpdir(), "tjommie-workflow-test-"));
    scratch.push(dir);
    const write = (name: string, text: string): string => { const file = join(dir, name); writeFileSync(file, text); return file; };
    const tail = "\non:\n  push:\n    branches: [main]\njobs:\n  a:\n    runs-on: windows-latest\n    steps:\n      - run: echo hi\n";
    const broken = parse(write("broken.yml", `name: Desktop acceptance (Phase 3: the installer, over R1)${tail}`));
    expect(broken.status).toBe(1);
    expect(broken.error).not.toBe("");
    const quoted = parse(write("quoted.yml", `name: "Desktop acceptance (Phase 3: the installer, over R1)"${tail}`));
    expect(quoted.status).toBe(0);
    expect(quoted.json).toMatchObject({ name: "Desktop acceptance (Phase 3: the installer, over R1)" });
  });
});
