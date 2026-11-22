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

// The lines of one TOML table: after its `[header]` line, up to the next line that opens any table (`[x]` or `[[x]]`), or the end of the
// file. null if there is no such table. Reading a value from this, and not from the whole file, is what keeps `authors = [...]` above
// the version from cutting the table short, and a dependency's `[dependencies.foo]` table (which has a `version` of its own) from being
// read as the crate's.
export function tableBody(toml: string, header: string): string | null {
  const opens = (line: string): string | null => /^\s*\[{1,2}\s*([^\]]*?)\s*\]{1,2}\s*(?:#.*)?$/.exec(line)?.[1]?.replace(/\s+/g, "") ?? null;
  const lines = toml.split(/\r?\n/);
  const start = lines.findIndex((line) => opens(line) === header && !/^\s*\[\[/.test(line));
  if (start === -1) return null;
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => opens(line) !== null);
  return (end === -1 ? rest : rest.slice(0, end)).join("\n");
}

// A table's own `version = "x"` (a TOML basic or literal string), or undefined.
function versionIn(body: string): string | undefined {
  return /^\s*version\s*=\s*(["'])([^"']+)\1\s*(?:#.*)?$/m.exec(body)?.[2];
}

// A crate's version: its own `version = "x"` in `[package]`, or the workspace's when it says `version.workspace = true` (or
// `version = { workspace = true }`).
export function crateVersion(manifest: string, workspaceVersion: string): string {
  const pkg = tableBody(manifest, "package");
  if (pkg === null) throw new Error("the crate manifest has no [package] table");
  const own = versionIn(pkg);
  if (own) return own;
  if (/^\s*version\s*\.\s*workspace\s*=\s*true\b/m.test(pkg) || /^\s*version\s*=\s*\{\s*workspace\s*=\s*true\s*\}/m.test(pkg)) return workspaceVersion;
  throw new Error("the crate manifest has no version");
}

export type VersionCheck = { packageVersion: string; crates: Record<string, string>; problems: string[] };

export function checkReleaseVersion(root: string): VersionCheck {
  const packageVersion = (JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as { version: string }).version;
  const workspace = readFileSync(join(root, "tjommie-rs", "Cargo.toml"), "utf8");
  const section = tableBody(workspace, "workspace.package");
  const workspaceVersion = section === null ? undefined : versionIn(section);
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
