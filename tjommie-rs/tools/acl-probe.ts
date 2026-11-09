// Cross-implementation probe for the Rust ACL port (T2.2). It runs the TypeScript daemon's ACL pass on the targets
// given as JSON on stdin and prints the pass result, so Rust tests can compare it and the resulting SDDL.
// It refuses any target outside the temp folder: this pass really changes ACLs.
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, relative, resolve } from "node:path";
import { windowsAclDriver, type AclTarget } from "../../packages/daemon/src/windows-acl.ts";

const targets: AclTarget[] = JSON.parse(readFileSync(0, "utf8"));
for (const { path } of targets) {
  const inside = relative(resolve(tmpdir()), resolve(path));
  if (!inside || inside.startsWith("..") || isAbsolute(inside)) throw new Error(`refusing a target outside ${tmpdir()}: ${path}`);
}
process.stdout.write(JSON.stringify(windowsAclDriver.runSync(targets)));
