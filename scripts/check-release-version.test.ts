import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { checkReleaseVersion, crateVersion, tableBody } from "./check-release-version.ts";

// Some of these start node (the command line), which on a busy PC can take longer than vitest's 10 s.
vi.setConfig({ testTimeout: 60_000 });

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

  // The review's F4. The gate reads each value from inside its own table, so a routine manifest edit cannot change what it reads.
  describe("reads a version only from its own table", () => {
    it("tableBody stops at the next table of either kind, and finds only a real table", () => {
      const toml = "[a]\nx = 1\n[b]\ny = 2\n[[c]]\nz = 3\n";
      expect(tableBody(toml, "a")).toBe("x = 1");
      expect(tableBody(toml, "b")).toBe("y = 2");
      expect(tableBody(toml, "c")).toBeNull(); // [[c]] is an array of tables, not the table c
      expect(tableBody(toml, "d")).toBeNull();
      expect(tableBody("[ workspace . package ] # why\nversion = \"1.0.0\"\n", "workspace.package")?.trim()).toBe('version = "1.0.0"');
      expect(tableBody("[a]\r\nx = 1\r\n[b]\r\n", "a")).toBe("x = 1");
    });

    it("finds the workspace version however many keys come before it", () => {
      const root = fixture("0.6.0", "0.6.0", { desktop: inherits("desktop"), daemon: inherits("daemon") });
      writeFileSync(
        join(root, "tjommie-rs", "Cargo.toml"),
        `[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nauthors = ["A"]\nkeywords = [\n  "a",\n  "b",\n]\nlicense = "MIT"\nversion = "0.6.0" # the release\nedition = "2024"\n\n[workspace.dependencies]\nserde = { version = "1" }\n`,
      );
      expect(checkReleaseVersion(root)).toEqual({ packageVersion: "0.6.0", crates: { desktop: "0.6.0", daemon: "0.6.0" }, problems: [] });
    });

    it("does not read a dependency's version as the crate's", () => {
      const manifest = `[dependencies.foo]\nversion = "1.0"\n\n[package]\nname = "desktop"\nversion.workspace = true\n\n[dependencies.bar]\nversion = "2.0"\nfeatures = ["x"]\n`;
      expect(crateVersion(manifest, "0.6.0")).toBe("0.6.0");
      // And when the crate has no version of its own, it is not found in a dependency's table either.
      expect(() => crateVersion(`[package]\nname = "x"\n\n[dependencies.foo]\nversion = "1.0"\n`, "0.6.0")).toThrow(/no version/);
    });

    it("reads the other ways a crate declares its version", () => {
      expect(crateVersion(`[package]\nname = "x"\nversion = { workspace = true }\n`, "0.6.0")).toBe("0.6.0");
      expect(crateVersion(`[package]\r\nname = "x"\r\nversion = "0.5.9" # pinned\r\n`, "0.6.0")).toBe("0.5.9");
      expect(crateVersion(`[package]\nname = "x"\nversion = '0.5.9'\n`, "0.6.0")).toBe("0.5.9");
      expect(crateVersion(`[package]\nname = "x"\nversion . workspace = true\n`, "0.6.0")).toBe("0.6.0");
    });

    it("still fails closed: no table, no version, or a workspace without one is an error, never a pass", () => {
      expect(() => crateVersion(`name = "x"\nversion = "1.0.0"\n`, "0.6.0")).toThrow(/no \[package\] table/);
      expect(() => crateVersion(`[package]\nname = "x"\n`, "0.6.0")).toThrow(/no version/);
      const root = fixture("0.6.0", "0.6.0", { desktop: inherits("desktop"), daemon: inherits("daemon") });
      writeFileSync(join(root, "tjommie-rs", "Cargo.toml"), `[workspace]\nmembers = ["crates/*"]\n\n[workspace.dependencies]\nserde = "1"\nversion = "0.6.0"\n`);
      expect(() => checkReleaseVersion(root)).toThrow(/no \[workspace\.package\] version/);
      // The command exits 1 on that, even with --warn: a gate that cannot read its inputs does not become a warning.
      const run = spawnSync(process.execPath, [script, "--root", root, "--warn"], { encoding: "utf8", windowsHide: true });
      expect(run.status).toBe(1);
    });
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
