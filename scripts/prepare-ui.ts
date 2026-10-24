import { cpSync, existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function prepareUiAssets(repo = resolve(fileURLToPath(new URL("..", import.meta.url)))): void {
  const wasm = join(repo, "packages", "transport", "wasm", "tjommie_noise_bg.wasm");
  if (!existsSync(wasm)) throw new Error("Build the local secure transport first: npm run build -w @tjommie/transport");
  const target = join(repo, "packages", "ui", "public");
  mkdirSync(target, { recursive: true });
  cpSync(wasm, join(target, "tjommie_noise_bg.wasm"));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) prepareUiAssets();
