import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function changelogSection(changelog: string, version: string): string {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => line === `## ${version}` || line.startsWith(`## ${version} — `));
  if (start < 0) throw new Error(`CHANGELOG.md has no section for ${version}.`);
  const end = lines.findIndex((line, index) => index > start && line.startsWith("## "));
  const body = lines.slice(start + 1, end < 0 ? undefined : end);
  if (!body.some((line) => line.trim() && !/^#{1,6}(?:\s|$)/.test(line.trimStart()))) {
    throw new Error("CHANGELOG.md section for " + version + " has no release notes.");
  }
  return `${lines.slice(start, end < 0 ? undefined : end).join("\n").trimEnd()}\n`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const version = process.argv[2];
  if (!version || process.argv.length !== 3) throw new Error("Usage: node scripts/changelog-section.ts <version>");
  try {
    process.stdout.write(changelogSection(readFileSync(new URL("../CHANGELOG.md", import.meta.url), "utf8"), version));
  } catch (error: unknown) {
    console.error(error instanceof Error ? error.message : "Could not read CHANGELOG.md.");
    process.exitCode = 1;
  }
}
