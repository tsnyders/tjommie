import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// `npm test` runs fast tests. `npm run test:real` (mode "real") runs the tests that drive the real CLIs.
export default defineConfig(({ mode }) => {
  const real = mode === "real";
  return {
    // The UI's tests render React components and import "@/..." paths the way the UI does.
    oxc: { jsx: { runtime: "automatic" } },
    resolve: { alias: { "@": fileURLToPath(new URL("./packages/ui", import.meta.url)) } },
    test: {
      execArgv: ["--disable-warning=ExperimentalWarning"],
      include: real ? ["packages/*/src/**/*.real.test.ts"] : ["packages/*/src/**/*.test.ts", "packages/ui/{lib,components}/**/*.test.{ts,tsx}"],
      exclude: real ? ["**/node_modules/**"] : ["**/node_modules/**", "**/*.real.test.ts"],
      testTimeout: real ? 600_000 : 10_000,
      hookTimeout: real ? 600_000 : 10_000,
      fileParallelism: !real,
    },
  };
});
