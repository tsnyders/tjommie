import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import ts from "typescript";

const root = resolve(import.meta.dirname, "..");
// Git excludes generated files and dependencies, while including new first-party source files.
const files = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.ts", "*.tsx", "*.mts", "*.cts"], { cwd: root, encoding: "utf8" })
  .split("\0")
  .filter(Boolean);
let violations = 0;
let scanned = 0;
for (const file of new Set(files)) {
  // Unstaged deletions remain in Git's index; there is no source left to inspect.
  if (!existsSync(resolve(root, file))) continue;
  scanned += 1;
  const source = ts.createSourceFile(file, readFileSync(resolve(root, file), "utf8"), ts.ScriptTarget.Latest, true);
  const visit = (node: ts.Node): void => {
    if (node.kind === ts.SyntaxKind.AnyKeyword) {
      const { line, character } = source.getLineAndCharacterOfPosition(node.getStart(source));
      console.error(`${file}:${line + 1}:${character + 1}: Explicit any types are forbidden.`);
      violations += 1;
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
}
if (violations > 0) process.exitCode = 1;
else console.log(`No explicit any types in ${scanned} TypeScript files.`);
