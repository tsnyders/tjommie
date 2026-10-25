import { execFile } from "node:child_process";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { chromium, type Browser } from "playwright-core";
import { rolldown } from "rolldown";
import { expect, it } from "vitest";
import { generateKeypair, initNoise, responder } from "./index.ts";

const bytes = readFileSync(new URL("../wasm/tjommie_noise_bg.wasm", import.meta.url));
initNoise(bytes);

it("the same WASM exchanges authenticated records between a real browser and the Node host", async () => {
  const bundle = await rolldown({ input: fileURLToPath(new URL("./index.ts", import.meta.url)) });
  const output = await bundle.generate({ format: "esm" }); await bundle.close();
  const code = output.output.find((chunk) => chunk.type === "chunk"); if (!code || code.type !== "chunk") throw new Error("Missing test bundle");
  const http = createServer((req, res) => {
    if (req.url === "/transport.js") res.writeHead(200, { "content-type": "application/javascript" }).end(code.code);
    else if (req.url === "/noise.wasm") res.writeHead(200, { "content-type": "application/wasm" }).end(bytes);
    else res.writeHead(200, { "content-type": "text/html" }).end('<!doctype html><title>Synthetic Noise fixture</title><script type="module">import * as api from "/transport.js"; window.noiseApi = api;</script>');
  });
  await new Promise<void>((resolve) => http.listen(0, "127.0.0.1", resolve));
  const address = http.address(); if (!address || typeof address === "string") throw new Error("Missing local fixture address");
  let browser: Browser | undefined;
  const host = generateKeypair(), psk = crypto.getRandomValues(new Uint8Array(32));
  const server = responder({ privateKey: host.privateKey, psk });
  try {
    browser = await chromium.launch({ channel: "chrome", headless: true });
    const page = await browser.newPage(); await page.goto(`http://127.0.0.1:${address.port}`);
    await page.waitForFunction("window.noiseApi !== undefined");
    const first = await page.evaluate(async ({ key, secret }) => {
      const api = (window as unknown as { noiseApi: typeof import("./index.ts") }).noiseApi;
      api.initNoise(new Uint8Array(await (await fetch("/noise.wasm")).arrayBuffer()));
      const device = api.generateKeypair();
      const client = api.initiator({ privateKey: device.privateKey, remotePublicKey: new Uint8Array(key), psk: new Uint8Array(secret) });
      (window as unknown as { noiseFixture: typeof client }).noiseFixture = client;
      device.privateKey.fill(0);
      return { publicKey: Array.from(device.publicKey), frame: Array.from(client.writeHandshake(new Uint8Array())) };
    }, { key: Array.from(host.publicKey), secret: Array.from(psk) });
    server.readHandshake(new Uint8Array(first.frame));
    expect(server.remotePublicKey).toEqual(new Uint8Array(first.publicKey));
    const confirmation = await page.evaluate((response) => {
      const client = (window as unknown as { noiseFixture: import("./index.ts").NoiseSession }).noiseFixture;
      client.readHandshake(new Uint8Array(response));
      return Array.from(client.encrypt(new TextEncoder().encode("synthetic confirmation")));
    }, Array.from(server.writeHandshake(new Uint8Array())));
    expect(new TextDecoder().decode(server.decrypt(new Uint8Array(confirmation)))).toBe("synthetic confirmation");
    const reply = server.encrypt(new TextEncoder().encode("synthetic result"));
    expect(await page.evaluate((response) => {
      const client = (window as unknown as { noiseFixture: import("./index.ts").NoiseSession }).noiseFixture;
      const result = new TextDecoder().decode(client.decrypt(new Uint8Array(response))); client.close(); return result;
    }, Array.from(reply))).toBe("synthetic result");
  } finally {
    server.close(); host.privateKey.fill(0); psk.fill(0);
    try { await browser?.close(); } finally {
      await new Promise<void>((resolve, reject) => http.close((error) => error ? reject(error) : resolve()));
    }
  }
});

it("loads the local WASM under the actual Electron Node runtime without a window", async () => {
  const require = createRequire(import.meta.url), electron: string = require("electron");
  const moduleUrl = new URL("../wasm/tjommie_noise.js", import.meta.url).href;
  const wasmPath = fileURLToPath(new URL("../wasm/tjommie_noise_bg.wasm", import.meta.url));
  const script = `import {readFileSync} from 'node:fs';
const api=await import(process.argv[1]); api.initSync({module:readFileSync(process.argv[2])});
const host=api.generateKeys(),device=api.generateKeys(),psk=crypto.getRandomValues(new Uint8Array(32));
const client=api.createInitiator(device.slice(0,32),host.slice(32),psk,new Uint8Array());
const server=api.createResponder(host.slice(0,32),psk,new Uint8Array());
try { server.readHandshake(client.writeHandshake(new Uint8Array())); client.readHandshake(server.writeHandshake(new Uint8Array()));
const result=server.decrypt(client.encrypt(new TextEncoder().encode('synthetic')));
if(new TextDecoder().decode(result)!=='synthetic')process.exitCode=1; else process.stdout.write('ok');
} finally {client.close();client.free();server.close();server.free();host.fill(0);device.fill(0);psk.fill(0);}`;
  const result = await promisify(execFile)(electron, ["--input-type=module", "-e", script, moduleUrl, wasmPath], {
    env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" }, windowsHide: true, timeout: 15_000, maxBuffer: 16_384,
  });
  expect(result.stdout).toBe("ok"); expect(result.stderr).toBe("");
});
