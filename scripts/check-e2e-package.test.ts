import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { installer, powershell, temp, windows } from "./ps-support.ts";

// installer/check-e2e-package.ps1: what the e2e job proves about the package it tests before it tests it. The files here are a few bytes with the
// strings in them; nothing is built or run.

vi.setConfig({ testTimeout: 90_000 });

const FEED = "https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml";
const DRIVER = "tjommie-ui-";
const MARKER = "TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41";
const filler = (text: string): Buffer => Buffer.concat([Buffer.alloc(200, 0), Buffer.from(text), Buffer.alloc(200, 0xff)]);

type Run = { status: number | null; output: string };
// The package folder, as the e2e job makes it: the desktop exe and the fixtures daemon side by side.
function check(desktop: Buffer, daemon: Buffer, options: { daemonName?: string; daemonFolder?: string } = {}): Run {
  const root = temp();
  const folder = join(root, "e2e-package");
  mkdirSync(folder);
  const daemonFolder = options.daemonFolder ? join(root, options.daemonFolder) : folder;
  mkdirSync(daemonFolder, { recursive: true });
  writeFileSync(join(folder, "tjommie-desktop.exe"), desktop);
  writeFileSync(join(daemonFolder, options.daemonName ?? "tjommie-daemon.exe"), daemon);
  const run = powershell(join(installer, "check-e2e-package.ps1"), ["-Desktop", join(folder, "tjommie-desktop.exe"), "-Daemon", join(daemonFolder, options.daemonName ?? "tjommie-daemon.exe")], { ...process.env });
  return { status: run.status, output: `${run.stdout}${run.stderr}` };
}

describe.skipIf(!windows)("the e2e package check", () => {
  it("passes the e2e build: a desktop exe with the test driver and no update feed, and the fixtures daemon beside it", () => {
    const run = check(filler(`strings ${DRIVER}1234 more`), filler(`a daemon ${MARKER}`));
    expect(run.status, run.output).toBe(0);
    expect(run.output).toMatch(/test driver and no update feed/);
  });

  it("fails a desktop exe with no test driver (the tests would have nothing to drive) and says so", () => {
    const run = check(filler("a build without the driver"), filler(MARKER));
    expect(run.status).not.toBe(0);
    expect(run.output).toMatch(/no test driver/);
  });

  it("fails a production build, which has the update feed, and a daemon that has the driver or is not the fixtures build", () => {
    const production = check(filler(`${FEED} ${DRIVER}`), filler(MARKER));
    expect(production.status).not.toBe(0);
    expect(production.output).toContain("update feed address");
    const real = check(filler(DRIVER), filler("a real daemon that starts real providers"));
    expect(real.status).not.toBe(0);
    expect(real.output).toMatch(/not\s+the\s+provider-fixtures\s+build/);
    const driven = check(filler(DRIVER), filler(`${MARKER} ${DRIVER}`));
    expect(driven.status).not.toBe(0);
    expect(driven.output).toMatch(/daemon binary contains the test driver/);
  });

  it("fails a daemon that is not beside the exe or is not named for what the tests look for, and reports every problem, not only the first", () => {
    const elsewhere = check(filler(DRIVER), filler(MARKER), { daemonFolder: "elsewhere" });
    expect(elsewhere.status).not.toBe(0);
    expect(elsewhere.output).toMatch(/beside\s+the\s+desktop\s+exe/);
    const named = check(filler(DRIVER), filler(MARKER), { daemonName: "daemon.exe" });
    expect(named.status).not.toBe(0);
    expect(named.output).toContain("tjommie-daemon.exe");
    const everything = check(filler(`${FEED}`), filler(`${DRIVER}`));
    expect(everything.status).not.toBe(0);
    for (const part of [/no\s+test\s+driver/, /update\s+feed\s+address/, /not\s+the\s+provider-fixtures/, /daemon\s+binary\s+contains\s+the\s+test\s+driver/]) expect(everything.output).toMatch(part);
  });

  it("fails a path that is not absolute and a missing file, as TJOMMIE_E2E_EXE itself would", () => {
    const relative = powershell(join(installer, "check-e2e-package.ps1"), ["-Desktop", "tjommie-desktop.exe", "-Daemon", "tjommie-daemon.exe"], { ...process.env });
    expect(relative.status).not.toBe(0);
    expect(`${relative.stdout}${relative.stderr}`).toMatch(/missing|absolute/);
  });
});
