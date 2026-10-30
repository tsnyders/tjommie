import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, writeFileSync, type Stats } from "node:fs";
import { basename, join } from "node:path";

export const packagedExecutable = "dist-desktop/installer/win-unpacked/Tjommie.exe";
export const provenanceFile = "dist-desktop/provenance.json";
const rebuild = "Run npm run package before npm run test:packaged.";
const generated = new Set(["node_modules", ".next", "out", "dist", "dist-desktop", "coverage", ".cache", ".turbo", "test-results", "playwright-report"]);

function isBuildInput(path: string): boolean {
  const parts = path.split("/");
  if (parts.some((part) => generated.has(part)) || path.endsWith(".tsbuildinfo") || basename(path) === "next-env.d.ts") return false;
  if (parts[0] === "packages") {
    // Package instructions and documentation do not feed the build. Source assets of all types do.
    return !parts.includes("docs") && !(parts.length === 3 && path.endsWith(".md"));
  }
  if (parts[0] === "scripts") return true;
  return parts.length === 1 && (path === "package.json" || path === "package-lock.json" || path === ".npmrc" ||
    /^tsconfig(?:\.[^/]+)?\.json$/.test(path) || /^[^/]+\.config\.[^/]+$/.test(path));
}

function digest(content: Buffer | string): string {
  return createHash("sha256").update(content).digest("hex");
}

// Hash actual worktree bytes, including untracked inputs. Git HEAD, index contents and mtimes are irrelevant.
export function buildInputHash(repo: string): string {
  const listing = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    cwd: repo, encoding: "utf8", windowsHide: true, maxBuffer: 16 * 1024 * 1024,
  });
  const entries: [string, string][] = [];
  for (const path of [...new Set(listing.split("\0").filter((path) => path && isBuildInput(path)))].sort()) {
    if (path.split("/").some((part) => part === ".." || part === ".") || path.startsWith("/")) throw new Error("Invalid packaging input path.");
    const file = join(repo, path);
    let stat: Stats;
    try { stat = lstatSync(file); }
    catch (error) {
      // `git ls-files --cached` includes tracked files deleted in the worktree. Their absence changes the hash.
      if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") continue;
      throw error;
    }
    if (!stat.isFile() || stat.isSymbolicLink()) throw new Error(`Packaging input must be a regular file: ${path}`);
    entries.push([path, digest(readFileSync(file))]);
  }
  for (const required of ["package.json", "package-lock.json"]) {
    if (!entries.some(([path]) => path === required)) throw new Error(`Missing packaging input: ${required}`);
  }
  return digest(JSON.stringify(["tjommie-packaging-inputs-v1", entries]));
}

type Artifact = [path: string, sha256: string];
type Provenance = { version: 1; inputsSha256: string; artifacts: Artifact[] };

// Hash every shipped byte, not just the Electron executable: app.asar, daemon/UI resources,
// dependency packages, the NSIS installer and its sidecars all live under this output directory.
function artifactHashes(repo: string): Artifact[] {
  const entries: Artifact[] = [];
  const root = "dist-desktop/installer";
  function visit(path: string): void {
    const file = join(repo, path);
    const stat = lstatSync(file);
    if (stat.isSymbolicLink()) throw new Error(`Packaged artifact must not be a link: ${path}`);
    if (stat.isDirectory()) {
      for (const name of readdirSync(file).sort()) visit(`${path}/${name}`);
    } else if (stat.isFile()) entries.push([path, digest(readFileSync(file))]);
    else throw new Error(`Packaged artifact must be a regular file: ${path}`);
  }
  try { visit(root); }
  catch (error) {
    throw new Error(`Packaged artifacts are missing or unreadable. ${rebuild}`, { cause: error });
  }
  entries.sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  if (!entries.some(([path]) => path === packagedExecutable)) throw new Error(`Packaged executable is missing. ${rebuild}`);
  const installers = entries.filter(([path]) => /^dist-desktop\/installer\/Tjommie-Setup-[^/]+\.exe$/.test(path));
  if (installers.length !== 1) throw new Error(`Exactly one packaged NSIS installer is required. ${rebuild}`);
  return entries;
}

// Called only after electron-builder succeeds. A source change during the build must not receive a stamp.
export function writePackagedProvenance(repo: string, initialInputsSha256: string): void {
  const inputsSha256 = buildInputHash(repo);
  if (inputsSha256 !== initialInputsSha256) throw new Error(`Build inputs changed during packaging. ${rebuild}`);
  const manifest: Provenance = {
    version: 1, inputsSha256, artifacts: artifactHashes(repo),
  };
  writeFileSync(join(repo, provenanceFile), `${JSON.stringify(manifest, null, 2)}\n`);
}

export function verifyPackagedProvenance(repo: string): void {
  let manifest: unknown;
  try { manifest = JSON.parse(readFileSync(join(repo, provenanceFile), "utf8")); }
  catch { throw new Error(`Packaged provenance is missing or unreadable. ${rebuild}`); }
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest) ||
      !("version" in manifest) || manifest.version !== 1 ||
      !("inputsSha256" in manifest) || typeof manifest.inputsSha256 !== "string" || !/^[a-f0-9]{64}$/.test(manifest.inputsSha256) ||
      !("artifacts" in manifest) || !Array.isArray(manifest.artifacts) || manifest.artifacts.length === 0 ||
      !manifest.artifacts.every((entry: unknown) => Array.isArray(entry) && entry.length === 2 &&
        typeof entry[0] === "string" && typeof entry[1] === "string" && /^[a-f0-9]{64}$/.test(entry[1]))) {
    throw new Error(`Packaged provenance is invalid. ${rebuild}`);
  }
  if (buildInputHash(repo) !== manifest.inputsSha256) throw new Error(`Packaged build is stale: build inputs changed. ${rebuild}`);
  if (JSON.stringify(artifactHashes(repo)) !== JSON.stringify(manifest.artifacts)) {
    throw new Error(`Packaged artifacts do not match their provenance. ${rebuild}`);
  }
}
