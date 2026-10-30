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

## Post-T11 release build (2026-10-26)

Built from integration commit `4624db1c02cf8d02a9e937f7822906aff3c60adf` with `DEFAULT_RELAY_URL` set to the deployed public relay. These are build artifacts only; neither installer nor APK was installed, and the phone and installed Tjommie app were untouched.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `dist-desktop/installer/Tjommie-Setup-0.3.0.exe` | 120879201 | `00c9628e9b9ffdb4c40cec15d5f0b444fce6328147f2bcba8fa11f4ecea85b09` |
| `dist-desktop/installer/Tjommie-Setup-0.3.0.exe.blockmap` | 121614 | `3298ddeb284c28330bc1fad0b91c810c919683218f4f92d1b2f7e5edcebea888` |
| `dist-desktop/installer/latest.yml` | 343 | `aa70bd6b597caf76f1d86f99ef988d45e5b8037a711a94bd79605cb6b049ede3` |
| `dist-mobile/Tjommie-Android-0.3.0.apk` | 5492588 | `3ba21b4bd12c40b1c9a65c9677b982303f599de7fbaae1746981cfc7941ae7c5` |

`npm run package` passed with production `appId=com.tsnyders.tjommie`, `productName=Tjommie`, version `0.3.0`, and an unsigned installer (`Get-AuthenticodeSignature: NotSigned`). The SHA-512 in `latest.yml` matches the installer. `npm run test:packaged` passed 1/1 against the unpacked executable, including the before-and-after provenance checks. An initial sandboxed smoke run could not inspect its test process; the rerun with OS process-inspection access passed. Windows was rebuilt after Android sync rewrote two tracked Gradle files' working-tree line endings, so the final desktop and mobile provenance both match the current source-input SHA-256 `e27a4a687d08b7ba3856c9896c69242672aa3e2e174d299623b9fd99dc31df4e`.

The owner-provided release script ran `npm run package:android -- --release` and passed. Android packaging verified 75 bundled UI assets and recorded a non-null Firebase configuration hash for project `aegis-orchestrator`, app `dev.tjommie.app`. Generated release `BuildConfig.PUSH` is `true`. `apksigner verify --print-certs --verbose` passed using APK Signature Scheme v2; the signing certificate SHA-256 is `39:16:A2:9F:49:9C:BE:18:FC:B7:F3:90:16:A8:86:6A:AA:AD:8D:0A:BB:F0:55:21:97:A2:DC:7C:03:F3:F7:50`. `aapt dump badging` confirms application ID `dev.tjommie.app`, `versionCode=3`, and `versionName=0.3.0`.

Before this documentation edit, `git status --short` was clean; no `google-services.json`, keystore, APK, or `dist-*` file is tracked. The controller's full gate for this commit passed separately (1,904 tests, 1 expected fail). Live FCM delivery, phone acceptance, a second-PC test, and installation remain unverified by this build-only pass.

## Owner PC installation (2026-10-26)

The owner authorized installing the 0.3.0 Windows build over the damaged 0.1.0 install. Before installation, no `Tjommie.exe` process or listener on ports 47100/47101 was present. The database opened read-only with `node:sqlite`: `PRAGMA user_version` was 7 and `PRAGMA quick_check` returned `ok`. There were no queued work items, but two old routine runs were still marked `running` (Inbox, started 06:00:17Z; Marketing, started 06:30:18Z). The controller confirmed these were left by the 08:31 SAST tray quit and explicitly ruled that startup should recover them as `interrupted`.

Before running the installer, `tjommie.db*` was copied to `%LOCALAPPDATA%\Tjommie-backup-2026-10-26-pre-0.3.0\`. SHA-256 matched source and copy for each file: `tjommie.db` (819200 bytes, `d6a1dc36b0e72e3a4d3d64dffed19db8f85e226ee641fd11a1ede68bd396f30e`), `tjommie.db-shm` (32768 bytes, `fd4c9fda9cd3f9ae7c962b0ddf37232294d55580e1aa165aa06129b8549389eb`), and the empty `tjommie.db-wal` (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).

The installer SHA-256 matched `00c9628e9b9ffdb4c40cec15d5f0b444fce6328147f2bcba8fa11f4ecea85b09`. `Start-Process` with `/S -Wait` exited 0 despite the missing old uninstaller. All 574 files from `dist-desktop/installer/win-unpacked` are present in `%LOCALAPPDATA%\Programs\Tjommie`; the installed directory has two additional files, `Uninstall Tjommie.exe` and `debug.log`. The uninstall entry `89b4e4b4-ac49-585f-800a-537f559f4641` says `Tjommie 0.3.0`, and the uninstaller exists. Desktop and Start Menu `Tjommie.lnk` shortcuts both target the installed `Tjommie.exe`. The entry's `InstallLocation` field is absent, while its uninstall command points to the installed uninstaller.

The installed `Tjommie.exe` was started and left running. Its daemon listens on `127.0.0.1:47100`. The database advanced to `user_version` 8, `quick_check` remains `ok`, and both named routine rows are now `interrupted` with `ended_at=2026-10-26T13:09:26.628Z`; there are no queued or running rows. The Windows Run entry `com.tsnyders.tjommie` targets the installed executable with `--hidden`. `desktop.log` was updated at 15:09:30 SAST and has subsequent positive startup signals with no startup-failed or daemon-start-failed line. Its recent history contains one untimestamped `The daemon stopped` line before those signals, so the log alone cannot date that older stop.

For database rollback, quit Tjommie and confirm the daemon port is closed, retain a separate copy of the current data, then restore the backed-up `tjommie.db*` files to `%LOCALAPPDATA%\Tjommie\data\` before starting a version compatible with `user_version` 7. A known-good older installer would be needed for an app rollback because the previous 0.1.0 installation was damaged. No rollback was performed.

## Owner phone installation (2026-10-26)

The owner approved replacing the debug-signed app on their Galaxy S25 Ultra (`SM-S938B`), including loss of its saved pairing. Wireless ADB was paired and connected to that phone. The release APK SHA-256 matched `3ba21b4bd12c40b1c9a65c9677b982303f599de7fbaae1746981cfc7941ae7c5` before installation. The existing `dev.tjommie.app` was versionName `0.1.0`, versionCode `1`, with signer certificate SHA-256 `aa5d1aa6b539481c5d805a6ff015c1e8e354b1d9311020304b5df2186107ac9a` (verified from its pulled base APK, then the temporary copy was removed).

`adb uninstall dev.tjommie.app` returned `Success`; `adb install` of the hash-verified 0.3.0 release APK then returned `Success`. The installed package reports versionName `0.3.0` and versionCode `3`. Its pulled base APK passed `apksigner verify --print-certs` with signer certificate SHA-256 `39:16:A2:9F:49:9C:BE:18:FC:B7:F3:90:16:A8:86:6A:AA:AD:8D:0A:BB:F0:55:21:97:A2:DC:7C:03:F3:F7:50`; the temporary copy was removed. The owner profile lists `dev.tjommie.app`, and a direct package dump reports `dev.tjommie.app.spike` absent.

One launcher event was sent with `adb shell monkey -p dev.tjommie.app -c android.intent.category.LAUNCHER 1`. The app process remained alive and its activity was top-resumed. AndroidRuntime logcat had no `FATAL EXCEPTION` or `Process: dev.tjommie.app` entry since launch. No notification permission was granted and no host pairing was performed; the owner will allow notifications and pair from PC Settings -> Devices.

## 0.3.1 release build (2026-10-27)

The done-and-failed notification changes were integrated before bump commit `3717a727d329e8a96c08788e85204fbf4b34890e`. The root and eight workspace versions, ten lockfile version fields, README installer line, and Android versionName were updated to 0.3.1; Android versionCode is 4. The lockfile has no dependency changes. Native sync now explicitly maps 0.3.0 to code 3 and 0.3.1 to code 4. `npm run typecheck` passed, including the no-`any` check across 461 TypeScript files.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `dist-desktop/installer/Tjommie-Setup-0.3.1.exe` | 120880219 | `54b9d24dbc75c45eaa232b581f0cc1003bc02349532be995febb80e465418104` |
| `dist-desktop/installer/Tjommie-Setup-0.3.1.exe.blockmap` | 121525 | `9708ad694c4a0e9402372b2bd56c51de168772b20822309392b0c3e7f15abbb7` |
| `dist-desktop/installer/latest.yml` | 343 | `f6d3bf3d376e0b2eede96770bc6b058e192837135e3c9fb49e941596c70948c0` |
| `dist-mobile/Tjommie-Android-0.3.1.apk` | 5492936 | `7ab690f7c57b96b3ac93333a7fac754843c1776f59ab2ed3292c1bbafc7b5fd8` |

The owner-provided Android release wrapper completed successfully with a non-null Firebase configuration hash and 75 verified UI assets. `aapt dump badging` reports `dev.tjommie.app`, versionCode `4`, versionName `0.3.1`; generated release `BuildConfig.PUSH` is `true`. `apksigner verify --print-certs --verbose` passed with certificate SHA-256 `39:16:A2:9F:49:9C:BE:18:FC:B7:F3:90:16:A8:86:6A:AA:AD:8D:0A:BB:F0:55:21:97:A2:DC:7C:03:F3:F7:50`.

`npm run package` passed with the production Tjommie identity and an unsigned Windows installer. The SHA-512 in `latest.yml` matches the installer. `npm run test:packaged` passed 1/1 using disposable app data and isolated accounts, including provenance checks before and after the smoke test. Desktop and Android provenance both record source-input SHA-256 `ddb3cf57d33752a2422c5617297bffcddab586defce29d5bf35cc6fe9f8b244a`. At build time, no 0.3.1 artifact had been installed; no relay was deployed, and nothing was pushed or published.

## Owner Windows and phone update to 0.3.1 (2026-10-27)

The owner authorized installation on both devices. Before the Windows update, `Tjommie.exe` was stopped, ports 47100 and 47101 had no listeners, and the database had no queued or running work items. The database opened read-only at `user_version` 8 with `quick_check=ok`. The 0.3.0 install was registered and its uninstaller existed.

Before installation, `tjommie.db*` was copied to `%LOCALAPPDATA%\Tjommie-backup-2026-10-27-pre-0.3.1\`. SHA-256 matched source and copy for `tjommie.db` (860160 bytes, `9a684ebe8bfa79b37408f16c8ac10eae9543b773511ad43ecc2192f9e1a1d726`), `tjommie.db-shm` (32768 bytes, `fd4c9fda9cd3f9ae7c962b0ddf37232294d55580e1aa165aa06129b8549389eb`), and the empty `tjommie.db-wal` (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).

The 0.3.1 installer hash matched the artifact table above; `Start-Process` with `/S -Wait` exited 0. All 574 files from `win-unpacked` are present in the installed directory (575 installed files including the uninstaller). The uninstall entry reports `Tjommie 0.3.1`, the uninstaller exists, and Start Menu and desktop shortcuts target the installed executable. Tjommie was started and left running: its daemon listens on `127.0.0.1:47100`, the Windows Run item uses `--hidden`, and the database advanced to `user_version` 9 with `quick_check=ok` and no queued or running work. The startup log includes the daemon listening marker; a broad scan of its recent history contains eight lines matching error-related words, so those historical lines were not treated as a current-start failure.

The Galaxy S25 Ultra (`SM-S938B`) was online over wireless ADB. Its existing `dev.tjommie.app` was release-signed 0.3.0, versionCode 3, with the same certificate as the 0.3.1 APK. The APK hash matched the artifact table above. `adb install -r` returned `Success`, updating in place without uninstalling or clearing app data. The installed package reports versionName `0.3.1`, versionCode `4`, and the pulled base APK passed `apksigner verify` with SHA-256 certificate `39:16:A2:9F:49:9C:BE:18:FC:B7:F3:90:16:A8:86:6A:AA:AD:8D:0A:BB:F0:55:21:97:A2:DC:7C:03:F3:F7:50`. The temporary pulled APK was removed and `dev.tjommie.app.spike` is absent. One launcher event opened `MainActivity`; the app process remained alive, the activity was top-resumed, and AndroidRuntime showed no fatal marker for this app in the two-minute launch window. No phone permission or pairing was changed.

For database rollback, quit Tjommie, confirm port 47100 is closed, retain a separate copy of current data, and restore the backed-up `tjommie.db*` files before starting an app version compatible with `user_version` 8. An older installer would be needed for a Windows app rollback. No rollback was performed.

## 0.3.1 release preparation (2026-10-30)

The `0.3.1` version bump and Android versionCode `4` were already merged into `main` by PR #4 at `8f2b04a2bc0763862dcbdf8b1c8b20c081875c1a`. The release worktree was created from that commit and `npm ci --no-audit --no-fund` passed (829 packages).

The production relay moved from Worker version `52405317-88e9-4782-b093-3ca58fbaea44` to `8cf0075d-4f8b-4112-9a4e-0278ff8d6872`. `GET /v1/health` returned HTTP 200 with `ok: true`. A synthetic host on a unique ID received a `ready` message advertising `approval`, `limit`, `routine_failed`, `done`, and `failed`; an unknown push kind returned `ok: false` without closing its socket. A read-only network check found an established HTTPS connection from the installed Tjommie process to an IP returned for the relay hostname. The app's on-screen Connected label was not inspected. The recorded rollback point is the previous Worker version.

The release worktree passed `npm run typecheck` (including no explicit `any` types across 461 TypeScript files), `npm run lint`, and `npm test` (180 files, 1,936 passed, 1 expected fail). Following the owner's updated direction, no local package build, packaged smoke test, tag, PR, or GitHub Release was made. A GitHub Actions build and release workflow is still to be planned.
