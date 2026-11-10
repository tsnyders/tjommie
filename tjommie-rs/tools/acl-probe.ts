// Cross-implementation probe for the Rust ACL port (T2.2). It runs the TypeScript daemon's ACL pass on the targets
// given as JSON on stdin and prints the pass result, so Rust tests can compare it and the resulting SDDL.
// It refuses any target that does not resolve inside the temp folder: this pass really changes ACLs. Both sides are
// resolved through junctions, symlinks and short names first, so no spelling or link can reach anything else.
import { readFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, relative, resolve } from "node:path";
import { windowsAclDriver, type AclTarget } from "../../packages/daemon/src/windows-acl.ts";

const targets: AclTarget[] = JSON.parse(readFileSync(0, "utf8"));
const temp = realpathSync.native(resolve(tmpdir()));
for (const { path } of targets) {
  let real: string;
  try { real = realpathSync.native(resolve(path)); } catch { throw new Error(`refusing a target that does not exist: ${path}`); }
  const inside = relative(temp, real);
  if (!inside || inside.startsWith("..") || isAbsolute(inside)) throw new Error(`refusing a target outside ${temp}: ${path}`);
}
process.stdout.write(JSON.stringify(windowsAclDriver.runSync(targets)));
