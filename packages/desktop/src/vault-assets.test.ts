import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

it("packages each trusted native entry asset beside the bundled main and keeps its bridge separate", () => {
  const packaging = readFileSync(new URL("../scripts/package.ts", import.meta.url), "utf8");
  for (const name of ["vault-entry.html", "vault-entry.js", "vault-entry.css", "vault-entry-preload.cjs"]) {
    expect(packaging).toContain(name); expect(readFileSync(new URL(`./${name}`, import.meta.url), "utf8").length).toBeGreaterThan(0);
  }
  const preload = readFileSync(new URL("./preload.cjs", import.meta.url), "utf8");
  expect(preload).not.toContain("private-login-submit"); expect(preload).not.toContain("private-login-init");
  expect(readFileSync(new URL("./vault-entry.html", import.meta.url), "utf8")).toContain("connect-src 'none'");
});
