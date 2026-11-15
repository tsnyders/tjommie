// Cross-implementation fixture for the login vault tests in tests/vault_storage.rs. Run by the test with node:
//   node --disable-warning=ExperimentalWarning vault_crossimpl.ts <write|read> <dataRoot>
// The JSON request arrives on stdin. The fixture drives the TypeScript vault the way the desktop does and prints JSON.
// It never prints a password: `read` only reports whether the stored secret matches the request.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { credentialNamespace } from "../../../../../packages/desktop/src/credential-transport.ts";
import { VaultRecords, WindowsLoginCredentials } from "../../../../../packages/desktop/src/vault-storage.ts";
import type { LoginEntry } from "../../../../../packages/protocol/src/vault.ts";

type WriteRequest = { entry: LoginEntry; username: string; password: string };
type ReadRequest = { username: string; password: string };

const [operation, dataRoot] = process.argv.slice(2);
if (!dataRoot || (operation !== "write" && operation !== "read")) {
  throw new Error("usage: vault_crossimpl.ts <write|read> <dataRoot>");
}
const databasePath = join(dataRoot, "data", "logins.sqlite");
const credentials = new WindowsLoginCredentials(dataRoot);
const records = new VaultRecords(databasePath);
try {
  if (operation === "write") {
    // Same order as login-vault.ts: reserve the reference, write the secret, then the record.
    const request = JSON.parse(readFileSync(0, "utf8")) as WriteRequest;
    const reference = records.prepare();
    await credentials.write(reference, { username: request.username, password: request.password });
    records.saved(request.entry, reference);
    process.stdout.write(JSON.stringify({ reference, namespace: credentialNamespace(dataRoot) }));
  } else {
    const request = JSON.parse(readFileSync(0, "utf8")) as ReadRequest;
    const database = new DatabaseSync(databasePath);
    const rows = database.prepare("SELECT data, reference FROM logins ORDER BY rowid").all() as { data: string; reference: string }[];
    database.close();
    let secretMatches = true;
    for (const row of rows) {
      const secret = await credentials.read(row.reference);
      secretMatches &&= secret?.username === request.username && secret?.password === request.password;
    }
    process.stdout.write(JSON.stringify({
      namespace: credentialNamespace(dataRoot),
      rows,
      pending: records.pending(),
      secretMatches,
      list: records.list(),
    }));
  }
} finally {
  records.close();
}
