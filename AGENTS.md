# Working in Tjommie

- Never use `any` as a TypeScript type.
- Every user-visible change adds a plain-English line under `## Unreleased` in `CHANGELOG.md`, in the same change.
- Releases: bump versions, add `<version>` → next versionCode in `packages/mobile/scripts/apply-native.mjs`, rename `## Unreleased` to `## <version> — <date>`, add a fresh empty `## Unreleased`, merge, then push tag `v<version>`; GitHub Actions builds and publishes.
- Builds and releases run on GitHub Actions only; don't package locally for release.
- The daemon is Rust, in `tjommie-rs`. Build it with `cargo build -p daemon --bin tjommie-daemon` inside `tjommie-rs`; the Electron UI starts that exe (set `TJOMMIE_DAEMON_EXE` to use another).
- Tests: `cargo test --workspace` inside `tjommie-rs` for the daemon (run cargo from that folder so its pinned toolchain applies); `npm test` (vitest) for the TypeScript packages.
- The TypeScript daemon and the pre-Rust structure live on the `legacy` branch, for reference.
