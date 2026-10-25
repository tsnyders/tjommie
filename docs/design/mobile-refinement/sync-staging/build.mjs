// Builds the staged kit into dist/: index.js (esbuild, deps external), the .d.ts tree (the repo's TypeScript), and styles.css
// (Tailwind 4 compiling the app's own globals.css against the copied sources). Reads the repo's node_modules; writes only here.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..", "..", "..");
const fromRepo = createRequire(join(repo, "package.json"));
const esbuild = createRequire(join(here, ".ds-sync", "package.json"))("esbuild");
const dist = join(here, "dist");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

await esbuild.build({
  entryPoints: [join(here, "src", "index.ts")],
  outfile: join(dist, "index.js"),
  bundle: true,
  format: "esm",
  jsx: "automatic",
  target: "es2022",
  tsconfig: join(here, "tsconfig.json"),
  external: ["react", "react-dom", "react/jsx-runtime", "lucide-react", "@base-ui/react", "@base-ui/react/*", "cn"],
  logLevel: "warning",
});

execFileSync(process.execPath, [fromRepo.resolve("typescript/bin/tsc"), "-p", join(here, "tsconfig.json")], { stdio: "inherit" });

const { compile } = await import(pathToFileURL(fromRepo.resolve("@tailwindcss/node")).href);
const { Scanner } = fromRepo("@tailwindcss/oxide");
const src = join(here, "src");
const compiler = await compile(readFileSync(join(src, "globals.css"), "utf8"), { base: src, onDependency: () => {} });
const candidates = new Scanner({ sources: [{ base: src, pattern: "**/*", negated: false }] }).scan();
writeFileSync(join(dist, "styles.css"), compiler.build(candidates));
console.log(`built dist/: index.js, .d.ts, styles.css (${candidates.length} Tailwind candidates)`);
