import { createServer } from "node:http";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { _electron, type ElectronApplication } from "playwright-core";
import { rolldown } from "rolldown";
import { expect, it, vi } from "vitest";

it("loads the real Noise WASM under the desktop CSP in an isolated dev Electron renderer", async () => {
  const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
  const csp = /const CSP =\s*"([^"]+)";/.exec(source)?.[1];
  if (!csp) throw new Error("The desktop CSP is missing.");
  const oldCsp = csp.replace(" 'wasm-unsafe-eval'", "");
  const bundle = await rolldown({ input: fileURLToPath(new URL("../../transport/src/index.ts", import.meta.url)) });
  const output = await bundle.generate({ format: "esm" }); await bundle.close();
  const code = output.output.find((chunk) => chunk.type === "chunk");
  if (!code || code.type !== "chunk") throw new Error("The Noise test bundle is missing.");
  const wasm = readFileSync(new URL("../../transport/wasm/tjommie_noise_bg.wasm", import.meta.url));
  const http = createServer((request, response) => {
    if (request.url === "/transport.js") response.writeHead(200, { "content-type": "application/javascript" }).end(code.code);
    else if (request.url === "/noise.wasm") response.writeHead(200, { "content-type": "application/wasm" }).end(wasm);
    else response.writeHead(200, { "content-type": "text/html", "content-security-policy": request.url === "/without-wasm" ? oldCsp : csp }).end(`<!doctype html><body>loading<script type="module">
      try {
        const noise = await import('/transport.js');
        noise.initNoise(new Uint8Array(await (await fetch('/noise.wasm')).arrayBuffer()));
        const pair = noise.generateKeypair(); pair.privateKey.fill(0);
        document.body.textContent = 'ready';
      } catch (error) { document.body.textContent = 'failed: ' + String(error); }
    </script></body>`);
  });
  const root = mkdtempSync(join(tmpdir(), "tjommie-csp-test-"));
  let electron: ElectronApplication | undefined;
  try {
    mkdirSync(join(root, "electron"));
    await new Promise<void>((resolve) => http.listen(0, "127.0.0.1", resolve));
    const address = http.address();
    if (!address || typeof address === "string") throw new Error("The test server has no port.");
    const url = `http://127.0.0.1:${address.port}/`;
    electron = await _electron.launch({ args: [fileURLToPath(new URL("./remote-csp-fixture.ts", import.meta.url))],
      env: { ...process.env, TJOMMIE_CSP_TEST_ROOT: root, TJOMMIE_CSP_TEST_URL: `${url}without-wasm` } });
    const page = await electron.firstWindow();
    await vi.waitFor(async () => expect(await page.locator("body").textContent()).toMatch(/^failed:/), { timeout: 10_000 });
    await page.goto(url);
    await vi.waitFor(async () => expect(await page.locator("body").textContent()).toBe("ready"), { timeout: 10_000 });
    const response = await page.request.get(url);
    expect(response.headers()["content-security-policy"]).toBe(csp);
    expect(csp).toContain("script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'");
    expect(csp).toContain("connect-src 'self' ws://127.0.0.1:*");
    expect(csp).not.toMatch(/connect-src[^;]*wss:/);
  } finally {
    try { await electron?.close(); }
    finally {
      try { await new Promise<void>((resolve) => http.close(() => resolve())); }
      finally { rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }); }
    }
  }
});
