import { expect, it } from "vitest";
import { changelogSection } from "./changelog-section.ts";

const changelog = "## Unreleased\n\n## 0.3.2 — 2026-10-30\n\n- A bot is done.\n\n## 0.3.0\n\n- Mobile data.\n";

it("extracts the 0.3.2 notes", () => {
  expect(changelogSection(changelog, "0.3.2")).toContain("- A bot is done.");
});

it("fails when the version is missing", () => {
  expect(() => changelogSection(changelog, "0.3.1")).toThrow("CHANGELOG.md has no section for 0.3.1.");
});

it("stops before the next version heading", () => {
  expect(changelogSection(changelog, "0.3.2")).not.toContain("Mobile data.");
});

it("handles CRLF line endings", () => {
  expect(changelogSection(changelog.replaceAll("\n", "\r\n"), "0.3.2")).toContain("- A bot is done.");
});

it("extracts the last section in the file", () => {
  expect(changelogSection("## 0.3.2\n\n- Last release note.\n", "0.3.2")).toContain("- Last release note.");
});

it("rejects a blank section body", () => {
  expect(() => changelogSection("## 0.3.2\n \n\t\n## 0.3.3\n- Next.\n", "0.3.2")).toThrow("CHANGELOG.md section for 0.3.2 has no release notes.");
});

it("rejects headings without release notes", () => {
  expect(() => changelogSection("## 0.3.2\n\n### What's new\n\n## 0.3.3\n- Next.\n", "0.3.2")).toThrow("CHANGELOG.md section for 0.3.2 has no release notes.");
});
