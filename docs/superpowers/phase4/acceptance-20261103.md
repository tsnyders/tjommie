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
