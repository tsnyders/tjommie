import { defineConfig } from "vitest/config";

// `npm test` runs fast tests. `npm run test:real` (mode "real") runs the tests that drive the real CLIs.
export default defineConfig(({ mode }) => {
  const real = mode === "real";
  return {
    test: {
      execArgv: ["--disable-warning=ExperimentalWarning"],
      include: [real ? "packages/*/src/**/*.real.test.ts" : "packages/*/src/**/*.test.ts"],
      exclude: real ? ["**/node_modules/**"] : ["**/node_modules/**", "**/*.real.test.ts"],
      testTimeout: real ? 600_000 : 10_000,
      hookTimeout: real ? 120_000 : 10_000,
      fileParallelism: !real,
    },
  };
});
