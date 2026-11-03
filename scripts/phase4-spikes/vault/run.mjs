import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

const root = mkdtempSync(join(tmpdir(), "tjommie-p4-vault-")), temporaryBase = resolve(tmpdir());
assert.equal(dirname(resolve(root)), temporaryBase); assert(basename(root).startsWith("tjommie-p4-vault-"));
const require = createRequire(import.meta.url), openssl = process.env.TJOMMIE_OPENSSL ?? "C:\\Program Files\\Git\\usr\\bin\\openssl.exe";
try {
  writeFileSync(join(root, "tls.cnf"), "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n");
  const certificate = spawnSync(openssl, ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-config", join(root, "tls.cnf"), "-keyout", join(root, "key.pem"), "-out", join(root, "cert.pem")], { windowsHide: true, stdio: "ignore", timeout: 20_000 });
  assert.equal(certificate.status, 0, "A disposable local TLS certificate could not be generated.");
  const env = { ...process.env, TJOMMIE_PHASE4_VAULT_ROOT: root }; delete env.ELECTRON_RUN_AS_NODE;
  const child = spawn(require("electron"), [join(import.meta.dirname, "probe.ts")], { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
  let output = ""; for (const stream of [child.stdout, child.stderr]) stream.on("data", (bytes) => { output = (output + bytes.toString()).slice(-32768); });
  const timer = setTimeout(() => child.kill(), 60_000);
  let code; try { code = await new Promise((done, reject) => { child.once("error", reject); child.once("exit", done); }); } finally { clearTimeout(timer); }
  process.stdout.write(output); assert.equal(code, 0, "The disposable HTTPS vault probe failed."); assert(output.includes("PHASE4_VAULT_RESULT"));
} finally {
  assert.equal(dirname(resolve(root)), temporaryBase); assert(basename(root).startsWith("tjommie-p4-vault-"));
  rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
}
