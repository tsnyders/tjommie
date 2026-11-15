import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { checkReleaseVersion, crateVersion } from "./check-release-version.ts";

// T8.3: the desktop and daemon crates that go into the installer must have the version the release is for (the root package.json's).
// The manifests are read as text, not through cargo, so this runs wherever vitest does.

const repo = resolve(import.meta.dirname, "..");
const script = join(import.meta.dirname, "check-release-version.ts");
const folders: string[] = [];
afterEach(() => { for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true }); });

// A tiny repository: a package.json and a tjommie-rs workspace with the given crate manifests.
function fixture(packageVersion: string, workspaceVersion: string, crates: Record<string, string>): string {
  const root = mkdtempSync(join(tmpdir(), "tjommie-version-test-"));
  folders.push(root);
  writeFileSync(join(root, "package.json"), JSON.stringify({ name: "tjommie", version: packageVersion }));
  mkdirSync(join(root, "tjommie-rs", "crates"), { recursive: true });
  writeFileSync(join(root, "tjommie-rs", "Cargo.toml"), `[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "${workspaceVersion}"\nedition = "2024"\n`);
  for (const [name, manifest] of Object.entries(crates)) {
    mkdirSync(join(root, "tjommie-rs", "crates", name), { recursive: true });
    writeFileSync(join(root, "tjommie-rs", "crates", name, "Cargo.toml"), manifest);
  }
  return root;
}
const inherits = (name: string): string => `[package]\nname = "${name}"\nversion.workspace = true\nedition.workspace = true\n`;
const own = (name: string, version: string): string => `[package]\nname = "${name}"\nversion = "${version}"\nedition = "2024"\n`;

describe("the release version check", () => {
  it("reads a crate's version from the workspace or from its own manifest", () => {
    expect(crateVersion(inherits("desktop"), "0.6.0")).toBe("0.6.0");
    expect(crateVersion(own("desktop", "0.5.9"), "0.6.0")).toBe("0.5.9");
    expect(() => crateVersion("[package]\nname = \"x\"\n", "0.6.0")).toThrow(/no version/);
  });

  it("passes when the desktop and daemon crates have the root package.json's version", () => {
    const root = fixture("0.6.0", "0.6.0", { desktop: inherits("desktop"), daemon: inherits("daemon") });
    expect(checkReleaseVersion(root)).toEqual({ packageVersion: "0.6.0", crates: { desktop: "0.6.0", daemon: "0.6.0" }, problems: [] });
  });

  it("names every crate that does not match, whichever way it declares its version", () => {
    const root = fixture("0.6.0", "0.5.1", { desktop: inherits("desktop"), daemon: own("daemon", "0.6.0") });
    const result = checkReleaseVersion(root);
    expect(result.problems).toEqual(["the desktop crate is 0.5.1, but package.json is 0.6.0"]);
    const both = checkReleaseVersion(fixture("0.6.0", "0.5.1", { desktop: inherits("desktop"), daemon: own("daemon", "0.4.0") }));
    expect(both.problems).toEqual(["the desktop crate is 0.5.1, but package.json is 0.6.0", "the daemon crate is 0.4.0, but package.json is 0.6.0"]);
  });

  it("fails a missing crate instead of passing it", () => {
    const root = fixture("0.6.0", "0.6.0", { desktop: inherits("desktop") });
    expect(() => checkReleaseVersion(root)).toThrow(/daemon/);
  });

  it("exits 1 on a mismatch, and with --warn reports it as a workflow warning and exits 0", () => {
    const root = fixture("0.6.0", "0.5.1", { desktop: inherits("desktop"), daemon: inherits("daemon") });
    const strict = spawnSync(process.execPath, [script, "--root", root], { encoding: "utf8", windowsHide: true });
    expect(strict.status).toBe(1);
    expect(strict.stderr).toContain("the desktop crate is 0.5.1, but package.json is 0.6.0");
    const lenient = spawnSync(process.execPath, [script, "--root", root, "--warn"], { encoding: "utf8", windowsHide: true });
    expect(lenient.status).toBe(0);
    expect(lenient.stdout).toContain("::warning::");
    expect(lenient.stdout).toContain("the desktop crate is 0.5.1, but package.json is 0.6.0");
  });

  it("holds for this repository today: the crates that ship are the version package.json says", () => {
    const result = checkReleaseVersion(repo);
    expect(result.problems).toEqual([]);
    expect(Object.keys(result.crates).sort()).toEqual(["daemon", "desktop"]);
  });
});
