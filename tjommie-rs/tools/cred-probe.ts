// Drives the TypeScript device keys (packages/desktop/src/device-keys.ts, the real Windows Credential Manager transport) for
// `crates/desktop-host/tests/cred_probe.rs`, which checks that Rust and TypeScript read each other's entries byte for byte.
// Only ever run with a disposable data root: the namespace comes from it, so a random test root never reaches the owner's
// `Tjommie/<production namespace>/...` entries.
//
//   node tools/cred-probe.ts seal   <dataRoot> <name> <base64url bytes>
//   node tools/cred-probe.ts read   <dataRoot> <name>     prints the key as base64url, or null
//   node tools/cred-probe.ts delete <dataRoot> <name>
import { createDeviceKeys } from "../../packages/desktop/src/device-keys.ts";

const [command, root, name, value] = process.argv.slice(2);
if (command === undefined || root === undefined || name === undefined) {
  console.error("usage: node tools/cred-probe.ts seal|read|delete <dataRoot> <name> [base64url bytes]");
  process.exit(2);
}
const keys = createDeviceKeys(root);
if (command === "seal" && value !== undefined) {
  await keys.seal(name, Buffer.from(value, "base64url"));
} else if (command === "read") {
  const bytes = await keys.read(name);
  console.log(bytes === null ? "null" : Buffer.from(bytes).toString("base64url"));
} else if (command === "delete") {
  await keys.delete(name);
} else {
  console.error(`Unknown command: ${command}`);
  process.exit(2);
}
