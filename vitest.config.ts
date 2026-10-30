import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// `npm test` runs fast tests; `test:real` drives real CLIs and the source desktop app.
// After `npm run package`, `npm run test:packaged` requires and verifies the Windows build.
export default defineConfig(({ mode }) => {
  const packaged = mode === "packaged";
  const real = mode === "real" || packaged;
  return {
    // The UI's tests render React components and import "@/..." paths the way the UI does.
    oxc: { jsx: { runtime: "automatic" } },
    resolve: { alias: { "@": fileURLToPath(new URL("./packages/ui", import.meta.url)) } },
    test: {
      execArgv: ["--disable-warning=ExperimentalWarning"],
      // Suppress the worker-count hint while retaining Vitest's default per-file isolation.
      experimental: { diagnostics: { isolate: false } },
      setupFiles: ["./scripts/vitest-setup.ts"],
      include: packaged ? ["packages/desktop/src/packaged.real.test.ts"] : real ? ["packages/*/src/**/*.real.test.ts", "packages/ui/visual/**/*.real.test.ts"] : ["packages/*/src/**/*.test.ts", "packages/mobile/scripts/**/*.test.ts", "packages/ui/{lib,components}/**/*.test.{ts,tsx}"],
      exclude: packaged ? ["**/node_modules/**"] : real ? ["**/node_modules/**", "**/packaged.real.test.ts"] : ["**/node_modules/**", "**/*.real.test.ts"],
      testTimeout: real ? 600_000 : 10_000,
      hookTimeout: real ? 600_000 : 10_000,
      fileParallelism: !real,
    },
  };
});
