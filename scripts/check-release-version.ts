import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The release workflow's version gate (T8.3): the crates that go into the installer, `desktop` (the app) and `daemon`, must be the version
// the release is for, which is the root package.json's. The manifests are read as text, so this needs no cargo and runs wherever node does.
//
//   node scripts/check-release-version.ts [--root <repository>] [--warn]
//
// Without --warn a mismatch exits 1 (the tag path). With --warn it is printed as a workflow warning and the exit is 0 (a dry run, which
// must be able to build a branch whose version has not been bumped yet).

const SHIPPED_CRATES = ["desktop", "daemon"] as const;

// A crate's version: its own `version = "x"`, or the workspace's when it says `version.workspace = true`.
export function crateVersion(manifest: string, workspaceVersion: string): string {
  const own = /^version\s*=\s*"([^"]+)"/m.exec(manifest);
  if (own?.[1]) return own[1];
  if (/^version\.workspace\s*=\s*true/m.test(manifest)) return workspaceVersion;
  throw new Error("the crate manifest has no version");
}

export type VersionCheck = { packageVersion: string; crates: Record<string, string>; problems: string[] };

export function checkReleaseVersion(root: string): VersionCheck {
  const packageVersion = (JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as { version: string }).version;
  const workspace = readFileSync(join(root, "tjommie-rs", "Cargo.toml"), "utf8");
  const section = /\[workspace\.package\][^[]*/.exec(workspace)?.[0] ?? "";
  const workspaceVersion = /^version\s*=\s*"([^"]+)"/m.exec(section)?.[1];
  if (!workspaceVersion) throw new Error("tjommie-rs/Cargo.toml has no [workspace.package] version");
  const crates: Record<string, string> = {};
  const problems: string[] = [];
  for (const name of SHIPPED_CRATES) {
    let manifest: string;
    try {
      manifest = readFileSync(join(root, "tjommie-rs", "crates", name, "Cargo.toml"), "utf8");
    } catch {
      throw new Error(`the ${name} crate has no Cargo.toml under tjommie-rs/crates`);
    }
    const version = crateVersion(manifest, workspaceVersion);
    crates[name] = version;
    if (version !== packageVersion) problems.push(`the ${name} crate is ${version}, but package.json is ${packageVersion}`);
  }
  return { packageVersion, crates, problems };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const rootIndex = args.indexOf("--root");
  const root = rootIndex >= 0 ? (args[rootIndex + 1] ?? ".") : resolve(import.meta.dirname, "..");
  try {
    const result = checkReleaseVersion(root);
    console.log(`package.json ${result.packageVersion}; ${Object.entries(result.crates).map(([name, version]) => `${name} ${version}`).join(", ")}`);
    if (result.problems.length > 0) {
      if (args.includes("--warn")) {
        for (const problem of result.problems) console.log(`::warning::${problem}`);
      } else {
        for (const problem of result.problems) console.error(problem);
        process.exitCode = 1;
      }
    }
  } catch (error: unknown) {
    console.error(error instanceof Error ? error.message : "Could not check the versions.");
    process.exitCode = 1;
  }
}
