import { appendFileSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Whether a run of .github/workflows/release.yml may publish a release (T8.3). Only a push of a v* tag may. Everything else, however it was
// started (a manual dispatch, even on a tag; a push to a branch; anything it does not recognise), is a dry run: it builds everything and
// uploads workflow artifacts and creates no release, pushes no tag and touches no latest.yml. When it cannot tell, it is a dry run.
//
//   node scripts/release-mode.ts   writes `dry_run=<true|false>` and `version=<package.json version>` to $GITHUB_OUTPUT
//                                  (or to the screen when that is not set).

export type ReleaseMode = { dryRun: boolean; reason: string };

// The version that goes into $GITHUB_OUTPUT. It comes from package.json on whatever branch ran the workflow, and GitHub reads a step's output
// file line by line, so a version with a newline in it could write an output line of its own (`dry_run=...`). It is held to the shape of a
// release version, the same one make-latest-yml.ts demands, and anything else stops the run before anything is written (the review's F5).
export function checkedVersion(text: unknown): string {
  if (typeof text !== "string" || !/^\d+\.\d+\.\d+$/.test(text)) throw new Error(`package.json's version is not a release version (x.y.z): ${JSON.stringify(text)}`);
  return text;
}

export function releaseMode(eventName: string | undefined, ref: string | undefined): ReleaseMode {
  if (eventName === "push" && typeof ref === "string" && /^refs\/tags\/v[^\s]*$/.test(ref)) return { dryRun: false, reason: `a push of the tag ${ref}` };
  return { dryRun: true, reason: `${eventName || "no event"} on ${ref || "no ref"} is not a push of a v* tag` };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const mode = releaseMode(process.env.GITHUB_EVENT_NAME, process.env.GITHUB_REF);
  try {
    const version = checkedVersion((JSON.parse(readFileSync(resolve(import.meta.dirname, "..", "package.json"), "utf8")) as { version?: unknown }).version);
    const lines = `dry_run=${mode.dryRun}\nversion=${version}\n`;
    if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, lines);
    else process.stdout.write(lines);
    console.error(mode.dryRun ? `Dry run: ${mode.reason}. Nothing will be published.` : `Release: ${mode.reason}.`);
  } catch (error: unknown) {
    // Nothing was written: the jobs that need the mode fail on a missing output, and none of them publishes.
    console.error(error instanceof Error ? error.message : "Could not read the version.");
    process.exitCode = 1;
  }
}
