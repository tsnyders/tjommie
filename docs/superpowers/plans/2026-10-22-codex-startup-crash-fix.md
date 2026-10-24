# Codex restored-usage startup crash

The installed desktop log repeatedly reported `RpcError: Usage identity is required` from Codex's notification callback through the runner's usage persistence. The daemon exited with code 1 until the desktop restart budget was exhausted.

The adapter accepted `thread/tokenUsage/updated` during `thread/start` or `thread/resume`, before the response supplied its resolved model. A synthetic regression reproduced the empty-model usage event in both startup paths. Replayed usage from a restored thread must not be recorded as new work.

The adapter now suppresses usage until this session sends new work, validates the resolved model, and enables usage before writing `turn/start` so early fresh notifications are retained. Usage remains enabled after Stop to keep genuinely late billing events. No provider profiles, pairing identities or historical usage rows were edited.

Validation:

- Both new regressions failed before the fix, with an empty-model usage event.
- All 42 Codex adapter tests passed after the fix, including fresh usage, deduplication, late usage after Stop and API-model pricing gates.
- The runner and parser suites passed in the focused run (151 tests); the initial adapter failure was corrected and its entire suite rerun.
- Root/UI TypeScript and the explicit-any check passed (353 TypeScript files).
- `npm run package` and mandatory `npm run test:packaged` passed.
- Silent desktop installation exited 0. All 570 installed payload files matched the tested package.
- The S25 reconnected to the installed desktop with its existing saved identity. No pairing code or provider request was sent by the verification script.
- A clean Android app restart showed Agents and established the authenticated LAN channel. The corrected desktop service retained the same PID throughout verification, with no new crash logged over several minutes.

Installer SHA-256: `7caa5b0aca8e52ec4d21c950dba804fe4133387b3ef153008dd8e2c8dac03248`. Build-input SHA-256: `9e305a95c5026f3cef85a09991952ec3e6a74153eccf2018717fc2cdc57c805d`.

The Android APK did not need changing for this desktop adapter failure. The desktop installer also contains the current original Quiet UI responsiveness changes.
