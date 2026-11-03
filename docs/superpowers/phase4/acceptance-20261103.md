# Phase 4 acceptance continuation

The owner requested all remaining Phase 4 implementation and acceptance work, then confirmed the phone is connected through wireless debugging. This extends the original source-only dispatch. Keep the installed app, account profiles and application data intact; test phone builds use `dev.tjommie.app.phase4test` and desktop builds use the existing isolated update-test identity. Builds run on GitHub Actions.

## Verified local speech runtime

- GitHub Actions run [41586392476](https://github.com/tsnyders/tjommie/actions/runs/41586392476) passed on commit `a27f842bd4bb5fc2c5e50e7a47c3ca91a14a04c9` in the isolated native probe branch. This branch contains the speech build recipe and verifier, not the complete Phase 4 implementation.
- The CPU x64 runtime was compiled from whisper.cpp commit `d1be6fde11ac6e0407606b4e42fe72d34add8037`. The downloaded artifact passed the source verifier's resource sizes and SHA-256 checks, including four notices. Runtime SHA-256: `7ae0960231f9d2f83af06253873f39df64821a6e41f0f0d389e8fcc82a194293`.
- A source `SpeechRuntime` acceptance harness used that executable and the pinned 147,964,211-byte English model in disposable acceptance storage. Windows speech synthesis produced 189,120 bytes of 16 kHz mono PCM. Actual CPU transcription finished in 13,761 ms and returned: “The project review is ready. Please check the files before sending this message.”
- No raw session files remained after the final transcript. Cancelling a second recording left no final transcript or session files. Removing the downloaded model returned status to `missing`.
- The first CI attempt compiled successfully but failed because the pinned upstream tree has a single root licence covering whisper and bundled ggml. The recipe now collects that root licence for both resource notices; the retry passed.

This proves the compiled executable and source runtime path on this Windows host. Packaged renderer microphone permissions, physical microphone quality, phone on-device recognition and editable-draft acceptance remain separate checks.

## Isolated artifact preparation

`phase4-acceptance.yml` builds test identities without production signing/Firebase secrets. Android packaging rejects the test flag outside a secret-free GitHub debug build. CI verifies the APK package identity and signature; the desktop artifact includes its commit and archive hash. Actual artifact downloads, installation and device acceptance must be recorded after their checks run.

Wireless ADB discovery confirmed a connected Android test device. No test app was installed during discovery and the owner's existing application data was not accessed.

## Continuation checks

- The combined local unit run completed successfully across 259 files and 2,293 assertions. All workspace typechecks, UI ESLint and the explicit-`any` scanner passed (627 TypeScript files). Subsequent focused checks passed 43/43, including private-console replay, failed process-stop handling and shortened inbox-path adoption.
- Independent read-only review found no blockers in the scoped worker-backed project-check launch/retirement path, vault command gate and main/daemon lifetimes, or private-console replay fix. This review did not provision Windows isolation or use credentials.
- The first full candidate was committed to the isolated `phase4-acceptance-20261103` branch at `30af53a6422e5c76ad25eac9c4b04d691123c7c6`; a follow-up was `d2ce28f89dca332999028480c0206545d951e42e`. Neither was merged to main or published as a release.
- Both candidate Android jobs passed packaging, application-ID and signature checks. Windows checks exposed shortened-path handling in protected shared snapshots and legacy inbox adoption. Local regressions reproduce both and pass after canonical-path comparison; a final exact-candidate CI run is still required.
- The first Android artifact, built by run [41590232469](https://github.com/tsnyders/tjommie/actions/runs/41590232469), was verified and installed separately for Android user 0 as `dev.tjommie.app.phase4test`, label **Tjommie Phase 4 Test**. APK SHA-256: `502f33c23f5b270f2e5988633679d029bcd14aab7215277f1761f54dbf20a271`. The owner's installed app was not replaced.
- The test app has paired over the local network to a disposable source daemon. Its relay is disabled, child PATH is empty, credentials are synthetic/in-memory and model startup throws; no model turn has started. Seven registered file fixtures include 200 MiB Markdown and PDF documents. Actual preview checks exposed a duplicate native `Content-Length` (`0, 216` for the small Markdown file) despite a correct encrypted RPC range response. The native fix and exact-artifact device rerun are pending; the existing viewer was not declared accepted.
- The phone's actual native speech status reports Android on-device recognition unavailable, no supported languages, system-managed model and microphone permission `prompt`. The app displays that state and does not substitute online recognition. No system speech settings were changed; spoken dictation is unverified on this phone.
- The hidden native HTTPS vault probe passed after fixing Chromium's replay of private console calls when Runtime observation resumes. It used synthetic in-memory credentials and a disposable certificate, not Windows Credential Manager. Exact evidence and limitations are in `p0-computer.md`.
- Global Windows worker ownership validation is implemented and covered by an executable C# regression. The current host token could not prove a complete cross-user process/SID snapshot, so required-worker review and its acceptance fail closed. Account provisioning and a reboot alone have not been demonstrated to resolve this capability requirement.

Desktop packaging, the actual Windows worker-SID acceptance and real isolated API review remain separate gates. No worker account, service, credential-store fixture, reboot or paid provider turn has been run by these continuation checks.

## Successful CI and physical range retest

GitHub Actions [41596329950](https://github.com/tsnyders/tjommie/actions/runs/41596329950) passed both jobs for `6c139b2e24521f88dff72e2e7ced2c7c822766e0`. Windows passed all typechecks/lint, **260 test files; 2,292 tests passed, one expected failure and two skips**, the hidden native HTTPS vault probe (9 ms fill), the pinned speech compilation and isolated desktop packaging. Android passed isolated packaging, identity/signature verification and `testDebugUnitTest`.

The corrected APK SHA-256 is `02b1d69da01697219c25bbb41ea8680f7aa1cfa32f19b1e9ca75f2260ccaa179`. CI's disposable debug signing certificate changed, so only the separate test app and its synthetic data were replaced for Android user 0. The production app was not replaced. Physical checks now show a single correct Content-Length, actual small Markdown and PDF rendering, a 200 MiB PDF rendered through bounded 256 KiB reads, PNG rendering, WAV playback/seeking and MP4 playback/seeking without the previous decode error. Deeper page/search/last-section checks are still being recorded.

The first native Save attempt remains a **failed acceptance check**: after the user returned from the picker, the UI reported cancellation and the uniquely named synthetic destination was not verified. The cause was forced reconnection after more than a minute hidden, which invalidated a healthy link and staged save. Source now probes the existing link with the bounded heartbeat before replacing it. Two additional RED/GREEN regressions fence old unanswered/overdue ping callbacks; **31/31** transfer/resume/heartbeat/host checks passed. This fix needs a new CI artifact and delayed-picker retest.

The normal worker-SID snapshot approach above is an intermediate rejected design. Read-only review found that snapshots do not enforce lifetime separation from a same-SID Full auto process. A separate review-only SID/profile is being implemented with paired purpose-bound configuration and acceptance; real provisioning remains unrun.
