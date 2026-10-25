# Phase 3 Windows and Android test delivery

Built on 2026-10-25 from `phase3-t12-integration` at BASE `2d3e485`, with the uncommitted T12 version and integration-test changes. These are local **pre-T11** test artifacts. `DEFAULT_RELAY_URL` is still `null`; rebuild both artifacts after the owner deploys the public relay and T11 sets its URL. The installer and unpacked executable have no Authenticode signature. The APK has Android debug signing and no Firebase configuration.

## Artifacts

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `dist-desktop/installer/Tjommie-Setup-0.3.0.exe` | 120877030 | `c9dbd26816a14a03f255108df4822d9348f62b92ad73b52843d24a2c3b5636c1` |
| `dist-desktop/installer/Tjommie-Setup-0.3.0.exe.blockmap` | 121685 | `7eb4d174e12b41090489f9c6e750a58ff6d46c5d031a5ebf8bb683a71245ce37` |
| `dist-desktop/installer/latest.yml` | 343 | `5c1587cda25573c070a876c8d7563eb6638474e41699fc19474f1e06e830fceb` |
| `dist-desktop/installer/win-unpacked/Tjommie.exe` | 246032896 | `f81cd8122cafbace0873b69f1d24b91f985fc5d234354d0f7505238886844b46` |
| `dist-mobile/Tjommie Android 0.3.0 debug.apk` | 6920901 | `71712f3a8a6d4d092a0ca9eafc54d4c192e8c7c89c8ec4e0d2d4b8f8937ac257` |

Both `dist-desktop/provenance.json` and `dist-mobile/provenance.json` record source-input SHA-256 `3e9cb030914d99c1f4b1e4604c48d7eb365f9907ce1c628e79bff47af0fe4c79`. The desktop provenance hashes every shipped file; `npm run test:packaged` checked it before and after running the unpacked executable. Android packaging verified 75 exported UI assets against the APK and recorded `firebaseConfigSha256: null`. `aapt dump badging` reports package `dev.tjommie.app`, `versionCode=3`, `versionName=0.3.0`; `apksigner verify --verbose` passed with APK Signature Scheme v2. Root, all eight workspaces, and the lockfile are at `0.3.0`.

## Completed checks

| Check | Result |
| --- | --- |
| `npm ci --no-audit --no-fund` | Passed; 829 packages installed. |
| `npm run typecheck` | Passed, including UI, transport, mobile, relay and the no-`any` check. |
| `npm run lint` | Passed. |
| `npm run check:no-any` | Passed across 445 TypeScript files. |
| `npm test` | 170 files, 1,826 tests passed. No known-flake rerun needed. |
| `npx vitest run --mode real packages/relay packages/daemon/src/relay-client.real.test.ts` | 2 files, 2 tests passed. |
| `npx vitest run --mode real packages/desktop/src/two-hosts.real.test.ts` | 1 test passed after the local-host state assertion and dead-setup removal. |
| `npm run package` | Passed; rebuilt after line-ending normalization so both provenance records match. |
| `npm run test:packaged` | 1 packaged smoke test passed against the final unsigned Windows build. |
| `npm run package:android` | Passed; debug APK and 75 matching UI assets. |
| `packages/mobile/android/gradlew.bat testDebugUnitTest --no-daemon` | 13 JVM tests passed, 0 failures. |
| `git diff --check` | Passed without warnings after line-ending normalization. |

The two-host test used host A as an isolated daemon and host B as an isolated source Electron app. A local HTTPS `wrangler dev` relay carried B's pairing because A advertised only `192.0.2.1` as its LAN address. B sent chat to a paused synthetic browser-enabled bot, saw “Runs on <PC>”, switched to “This PC” and back, then displayed “This PC removed this device” within 2 seconds of A's revoke. The test stopped its own processes, removed disposable roots and deleted its namespaced device-key entries from Windows Credential Manager. No provider turn or paid call was made.

The full `npm test`, focused relay-real gate and Android JVM suite ran in the initial T12 pass. After fix round 1, typecheck, lint and the changed two-host test were rerun, followed by both package builds and the packaged smoke check.

## T0 background verdict

The Android foreground-service spike **failed** its 95% gate: screen off with the service delivered 23/30 scheduled RPCs (76.7%) over 30 minutes, stopped delivering about seven minutes before the end, and delivered 0/15 scheduled RPCs during 14m41s of forced deep idle; motion ended that pass 20 s early. The spike's battery UID estimate rose about 59 mAh during the service screen-off pass (roughly 120 mAh/h). Without the service, screen off delivered 0/30. A Wi-Fi-to-data-to-Wi-Fi attempt failed while off LAN and recovered after Wi-Fi returned; it did not prove mobile-data relay use. The no-service deep-idle and switch baselines were skipped after the failure. Samsung app sleeping was not separately exercised; a battery-optimization exemption and wakelock were not tested. Phase 3 therefore hides the “Always connected” toggle and uses reconnect on return plus FCM push for background liveness.

## Test data and delivery limits

All test roots, pairing codes, bot messages and local relay state were disposable. The installed Tjommie process, its credential target and data paths, the phone, live Cloudflare/Firebase accounts and GitHub releases were not touched. `screens.real.test.ts` was skipped because it uses real accounts and makes paid calls. `phone-layout.real.test.ts` was not run, so tracked phone screenshots were not overwritten.

Three throwaway `Tjommie/<namespace>/remote-host` Credential Manager entries from an early RED run were left behind. With the owner's explicit OK at 23:12, the exact `fa212570ff6a2a7ff717b5b6f4df3fba`, `d45633133c06b0192fd20bbcc57eb876` and `6c42467536fc7f5ddfabc2477c5e98d8` targets were deleted one at a time; `cmdkey /list` then showed only the untouched installed-app target `Tjommie/73df24cc826296a686a69d3731b30743/remote-host`.

`updater.real.test.ts` was not rerun at integration: its safety snapshot reads `%LOCALAPPDATA%\Tjommie` and `%LOCALAPPDATA%\Programs\Tjommie`, which this task's hard limits prohibit. T6's prior updater proof remains the available automated evidence; update installation remains an owner-run check.

S25 acceptance, a real second PC, live mobile-data relay, live FCM push and update installation remain owner-run checks. A relay deployment URL and Firebase configuration are needed before the final rebuild; a release keystore is needed for a release-signed APK. The ops agent owns the commit, PR and GitHub Release draft, with publication after owner approval.
