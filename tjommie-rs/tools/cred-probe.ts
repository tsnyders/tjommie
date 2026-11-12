// Cross-implementation probe for the Rust Credential Manager port (T2.1). It runs the real TypeScript modules so
// Rust tests can check that both sides read each other's entries and derive the same target names.
// Usage: node cred-probe.ts <operation> <root> <id> [login reference, for "targets"]. Secrets travel on stdin and stdout only, never in argv.
import { readFileSync } from "node:fs";
import { WindowsAccountCredentials, type CredentialTransport } from "../../packages/daemon/src/account-credentials.ts";
import { openDb } from "../../packages/daemon/src/db.ts";
import { loadRelayIdentity } from "../../packages/daemon/src/relay-identity.ts";
import { loadRemoteIdentity } from "../../packages/daemon/src/remote-identity.ts";
import { WindowsLoginCredentials, type VaultSecret } from "../../packages/desktop/src/vault-storage.ts";

const [operation, root, id, login = "00000000-0000-4000-8000-000000000000"] = process.argv.slice(2);
if (!operation || !root || !id) throw new Error("usage: cred-probe.ts <operation> <root> <id> [login reference]");
const stdin = (): string => readFileSync(0, "utf8");
const print = (value: unknown): void => { process.stdout.write(JSON.stringify(value)); };

/** Captures the target a module asks for, then fails the call so nothing is read or written. */
async function targetOf(use: (transport: CredentialTransport) => Promise<unknown>): Promise<string> {
  let seen = "";
  await use(async ({ target }) => { seen = target; throw new Error("probe"); }).catch(() => {});
  if (!seen) throw new Error("no target requested");
  return seen;
}

switch (operation) {
  case "targets":
    print({
      account: new WindowsAccountCredentials(root).reference(id),
      login: await targetOf((transport) => new WindowsLoginCredentials(root, transport).read(login)),
      remoteHost: await targetOf((credentials) => loadRemoteIdentity({ db: openDb(":memory:"), root, name: "probe", credentials, generate: () => ({ privateKey: new Uint8Array(32), publicKey: new Uint8Array(32) }) })),
      relayLogin: await targetOf((credentials) => loadRelayIdentity({ root, credentials })),
    });
    break;
  case "account-read": print(await new WindowsAccountCredentials(root).read(id)); break;
  case "account-write": await new WindowsAccountCredentials(root).write(id, stdin()); print(null); break;
  case "account-delete": await new WindowsAccountCredentials(root).delete(id); print(null); break;
  case "login-read": print(await new WindowsLoginCredentials(root).read(id)); break;
  case "login-write": {
    let secret: VaultSecret;
    // A SyntaxError can quote its input, and the Rust test prints stderr: nothing from stdin may reach it.
    try { secret = JSON.parse(stdin()); } catch { process.exit(1); }
    await new WindowsLoginCredentials(root).write(id, secret); print(null); break;
  }
  case "login-delete": await new WindowsLoginCredentials(root).delete(id); print(null); break;
  default: throw new Error(`unknown operation ${operation}`);
}
