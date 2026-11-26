import { createHash } from "node:crypto";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Writes the release's `latest.yml` in exactly the format electron-builder writes, which R1's electron-updater reads and the Rust updater
// (M6) parses strictly: the installer's name, SHA-512 (base64) and size, and when it was made. No block map is published (T8.3), so there is
// no block-map field and electron-updater falls back to a full download.
//
//   node scripts/make-latest-yml.ts --installer <Tjommie-Setup-x.y.z.exe> --version <x.y.z> --out <latest.yml> [--release-date <ISO time>]
//
// scripts/release-assets.test.ts holds this to the real file published with v0.5.1.

export type LatestFields = { version: string; installerName: string; sha512: string; size: number; releaseDate: string };

const VERSION = /^\d+\.\d+\.\d+$/;
const ISO = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

// The text, byte for byte as electron-builder writes it: LF line endings, a trailing newline, and the time in single quotes (a bare time
// would be read as a YAML timestamp).
export function renderLatestYml(fields: LatestFields): string {
  return [
    `version: ${fields.version}`,
    "files:",
    `  - url: ${fields.installerName}`,
    `    sha512: ${fields.sha512}`,
    `    size: ${fields.size}`,
    `path: ${fields.installerName}`,
    `sha512: ${fields.sha512}`,
    `releaseDate: '${fields.releaseDate}'`,
    "",
  ].join("\n");
}

function argument(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

export function makeLatestYml(args: string[]): string {
  const installer = argument(args, "--installer");
  const version = argument(args, "--version");
  const out = argument(args, "--out");
  const releaseDate = argument(args, "--release-date") ?? new Date().toISOString();
  if (!installer) throw new Error("--installer is required.");
  if (!version) throw new Error("--version is required.");
  if (!out) throw new Error("--out is required.");
  if (!VERSION.test(version)) throw new Error(`The version must be x.y.z, not '${version}'.`);
  if (!ISO.test(releaseDate)) throw new Error(`--release-date must be an ISO time like 2026-12-01T10:00:00.000Z, not '${releaseDate}'.`);
  const expectedName = `Tjommie-Setup-${version}.exe`;
  if (basename(installer) !== expectedName) throw new Error(`The installer must be named ${expectedName}, not ${basename(installer)}.`);
  if (!statSync(installer).isFile()) throw new Error(`${installer} is not a file.`);
  const bytes = readFileSync(installer);
  const sha512 = createHash("sha512").update(bytes).digest("base64");
  writeFileSync(out, renderLatestYml({ version, installerName: expectedName, sha512, size: bytes.length, releaseDate }));
  return `Wrote ${out}: ${expectedName}, ${bytes.length} bytes, sha512 ${sha512}`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(makeLatestYml(process.argv.slice(2)));
  } catch (error: unknown) {
    console.error(error instanceof Error ? error.message : "Could not write latest.yml.");
    process.exitCode = 1;
  }
}
