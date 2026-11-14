# Rollback-compatibility fixtures

These files pin what the 0.4.2 TypeScript daemon (`packages/daemon`, kept on the `legacy` branch) wrote or ran, so the Rust
daemon stays compatible with it. A rollback means reinstalling 0.4.2 over a data root that Rust has used: 0.4.2 must still
read every secret Rust stored and accept every ACL, worker configuration and relay key Rust left behind.

All but `worker-acceptance.json` were captured on 2026-11-13 by running the TypeScript modules once, before
`packages/daemon` was removed, with a throwaway script (it stays on `legacy`; it never touched Credential Manager and only
changed ACLs on folders it made in `%TEMP%`). `worker-acceptance.json` is regenerated, not hand-made:
`tjommie-rs/tools/capture-worker-acceptance.mjs` lifts the relevant lines out of a checkout of `origin/legacy` and runs them
on synthetic inputs (its header says how to run it; it needs Node 24, `npm ci` and that checkout, and it regenerates the file
byte for byte). **Do not change any of them unless rollback compatibility with 0.4.2 is deliberately dropped.** Each JSON
file says how it was made in its `_provenance` field.

One thing in `worker-acceptance.json` is deliberately **not** what the Rust code runs: the probe's Node wrapper. Its
`spawnSync` gives PowerShell `timeout:15000` in 0.4.2; the Rust one gives it `PROBE_INNER_TIMEOUT` (120 s), because on a
real runner PowerShell took 12 to 19 s to start under a worker's environment and the probe was killed before it printed
anything. The fixture keeps what 0.4.2 ran, and `windows_worker_acceptance.rs` pins the difference: the Rust wrapper must
equal the 0.4.2 one with that single value replaced, and the holder's wrapper must still be byte for byte 0.4.2's. The
receipts carry neither wrapper, so they, and rollback to 0.4.2, are unaffected.

| File | Pins | Checked by |
| --- | --- | --- |
| `credential-entries.json` | Credential Manager target names, blob encoding and `CREDENTIALW` shape (type, persist, user name) | `cred_roundtrip.rs` |
| `relay-identity.json` | A relay login key entry 0.4.2 minted, with the public key and signature it gives | `relay_identity.rs` |
| `acl-pass.json` | What the Codex data ACL pass reports and leaves on a folder tree | `windows_acl_driver.rs` |
| `worker-scripts.json` | The fixed Node programs a Windows worker runs, and the policy observer's sha256 | `windows_worker_launch.rs`, `windows_worker_boundary.rs` |
| `worker-acceptance.json` | The two acceptance receipts (`worker-acceptance.json`, `review-worker-acceptance.json`) 0.4.2's `accept-worker.ts` wrote for a synthetic pair, byte for byte, with the pair's configurations and hashes, and the two `node -e` programs it ran its probe and peer holder with | `windows_worker_acceptance.rs` |

The Noise prologue and the pairing and resume handshakes are still checked live against `packages/client`, which R1 keeps:
`remote_interop.rs` drives it through `tools/remote-probe.ts`.
