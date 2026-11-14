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

Two things in `worker-acceptance.json` are deliberately **not** what the Rust code runs, both in the probe's `node -e`
program. The fixture keeps what 0.4.2 ran, and `windows_worker_acceptance.rs` pins the difference: the Rust probe must equal
the 0.4.2 one with exactly these two things replaced and nothing else, and the holder's program must still be byte for byte
0.4.2's. The receipts carry neither program, so they, and rollback to 0.4.2, are unaffected: the receipt bytes are still
checked against the fixture, and `ownerDesktopDenied` is the same field in the same place.

1. **The time limit.** The wrapper's `spawnSync` gives PowerShell `timeout:15000` in 0.4.2; the Rust one gives it
   `PROBE_INNER_TIMEOUT` (120 s), because on a real runner PowerShell took 12 to 19 s to start under a worker's environment
   and the probe was killed before it printed anything.
2. **The desktop test** (`DesktopDenied` in the probe's C#). 0.4.2 counted the desktop as denied only if
   `OpenInputDesktop(0,false,0x0108)` returned no handle with Win32 error 5. That rule could never be met by a correctly
   isolated worker. A worker runs on its own private, non-interactive window station, and from there `OpenInputDesktop`
   fails with error 1 (`ERROR_INVALID_FUNCTION`) for every access mask: the call is structurally impossible, whatever the
   permissions, so it says nothing about them. A diagnostic run on a GitHub-hosted Windows Server 2025 runner (2026-11-14)
   showed it. The runner's own user (`runneradmin`, session 2, on the visible `WinSta0`) opens the input desktop, so the
   positive control works there. Both workers, on their private `TjommieWorker-...` station with desktop `default`, got no
   handle and error 1 from `OpenInputDesktop` with every mask tried; no handle and error 5 (`ERROR_ACCESS_DENIED`) from
   `OpenWindowStation("WinSta0", WINSTA_READATTRIBUTES)`, which is the real boundary; and could open their own `default`
   desktop on their own station, as designed. The launch is identical to 0.4.2's, so this was always so. The Rust probe
   now counts the desktop as denied only if **both** hold: `OpenInputDesktop(0,false,0x0108)` yields no handle (with any
   error; a handle is a failure), and `OpenWindowStation("WinSta0", false, WINSTA_READATTRIBUTES)`, the weakest access
   there is, fails with exactly error 5 (a handle, or any other error, is a failure). That is stricter than 0.4.2's
   rule, never weaker. `accept-worker` also runs the owner's side itself before anything else: it opens `WinSta0` and the
   input desktop as the owner and stops with a plain error if it cannot, so a worker's refusal always means the target is
   there and the worker was kept out.

| File | Pins | Checked by |
| --- | --- | --- |
| `credential-entries.json` | Credential Manager target names, blob encoding and `CREDENTIALW` shape (type, persist, user name) | `cred_roundtrip.rs` |
| `relay-identity.json` | A relay login key entry 0.4.2 minted, with the public key and signature it gives | `relay_identity.rs` |
| `acl-pass.json` | What the Codex data ACL pass reports and leaves on a folder tree | `windows_acl_driver.rs` |
| `worker-scripts.json` | The fixed Node programs a Windows worker runs, and the policy observer's sha256 | `windows_worker_launch.rs`, `windows_worker_boundary.rs` |
| `worker-acceptance.json` | The two acceptance receipts (`worker-acceptance.json`, `review-worker-acceptance.json`) 0.4.2's `accept-worker.ts` wrote for a synthetic pair, byte for byte, with the pair's configurations and hashes, and the two `node -e` programs it ran its probe and peer holder with (the Rust probe differs from the first in two pinned places, above) | `windows_worker_acceptance.rs` |

The Noise prologue and the pairing and resume handshakes are still checked live against `packages/client`, which R1 keeps:
`remote_interop.rs` drives it through `tools/remote-probe.ts`.
