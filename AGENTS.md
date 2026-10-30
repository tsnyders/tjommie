# Working in Tjommie

- Never use `any` as a TypeScript type.
- Every user-visible change adds a plain-English line under `## Unreleased` in `CHANGELOG.md`, in the same change.
- Releases: bump versions, add `<version>` → next versionCode in `packages/mobile/scripts/apply-native.mjs`, rename `## Unreleased` to `## <version> — <date>`, add a fresh empty `## Unreleased`, merge, then push tag `v<version>`; GitHub Actions builds and publishes.
- Builds and releases run on GitHub Actions only; don't package locally for release.
