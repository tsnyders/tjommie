# Native browser follow-up

Status: source integration, final profile/iframe fixes, package gate, installed native UI and all nine installed provider workflows verified. The owner subsequently reported "Everything works now"; individual third-party sites were not itemized.

The browser is now a desktop-owned Electron WebContentsView. Human input and media use that native page directly; the bot's browser tools operate on the same WebContents. Details/Browser switching, panel resizing, expansion, caption spacing and the rail padding fix remain in place. Profiles are isolated per bot below protected application data. Old Chrome profiles are retained without cookie import.

Paseo agents implemented the engine, UI and 26-tool automation adapter. Independent reviewers reported findings without editing files. The private desktop-host capability is separate from the renderer token and travels over the startup pipe. No production remote-debugging TCP listener is opened. Tool cancellation has a separate private control lane so four busy normal workers cannot delay Stop.

## Current evidence

- Full fast suite: 1,152 tests in 94 files passed after the four engine review fixes. The final Windows canonical-path correction passed all 16 focused helper/engine/protocol tests, root typecheck and no-any.
- Root/UI typechecks and no-any across 261 TypeScript files passed. Lint and static UI build also passed.
- Disposable native engine and adapter: three real Electron tests passed. They cover native input, isolated guest privileges, per-bot storage, restart persistence, POST/opener-preserving in-panel popups, locally generated VP8 WebM playback, audio, file upload/download, dialogs, takeover, child-frame compatibility and profile deletion/pruning.
- The combined source desktop panel passed its final real test (27.35 seconds), including Details, resize, expanded/narrow layouts, hiding/reopening, profile isolation and browser disable.
- Screenshot capture uses PrintWindow for the fixture's verified window handle and process ID. Matching synthetic page pixels prove the native child view is present in the composed window; BrowserWindow.capturePage alone omits that child surface.
- Final installed native UI acceptance passed in 19.66 seconds. An earlier capture found the fixture minimized (a title-bar-only image); the harness now restores only its owned window before capture. One intervening run lost its daemon WebSocket and failed; two subsequent complete runs passed. The final package was rebuilt for the harness change and passed the mandatory packaged gate (one test, no skips).
- All nine final installed workflows passed: provider readiness, cross-provider exchange, steering, Stop, actual running-command Stop, persistent approval rules, native browser tools, routine approval and a scheduled routine with the window closed to the tray. Stop took 711 ms and actual-command Stop took 891 ms in this run.
- Native browser tools reached the installed app's own isolated guest, with Node disabled and one top-level BrowserWindow. Installed payload bytes were checked against package provenance before acceptance.
- The metadata-only console observer sampled every 50 ms throughout the nine-workflow run: zero baseline console windows, zero new console windows, zero observer errors. Sampling is not proof that no shorter-than-interval flash is possible.

The coding agents performed no real Google account, consent or credential interaction. Google website sign-in and YouTube site-specific compatibility were not independently verified by the agents. The owner's subsequent successful-use report is recorded above. The separate Google API groundwork is disabled in production and is not a website-session solution.

## Limits

Native browser tools require the desktop app to remain running, including in the tray. Camera, microphone, devices and screen-capture permissions are denied. Tool scripts run only in the page; Playwright/Node `run_code` is not provided. Tool output and uploads/downloads are bounded. Arbitrary page-created future timers are page behavior and are not a promise of complete script cancellation.

One first-failure disposable test directory, `tjommie-native-7RMr6d`, remains because automatic approval review blocked cleanup. Its owned processes were stopped; no alternate deletion was attempted.

Final installer: `dist-desktop/installer/Tjommie Setup 0.1.0.exe`, SHA-256 `11ab25f88c2c1058f0c8ce058f6070652d66cc37e4cac60e129b98242a3cdf39`.

Build inputs SHA-256: `ab1d5630ebbf9881372c3ee233b8ce9d5f10aa267b20f79c1097e584095ed726`. This records actual worktree bytes, including inputs that were uncommitted at build time. Subsequent commit and merge state is recorded in Git.

Installed executable SHA-256: `04c135e3a226e8b40c98440ffe694084c2efd0411df9e7a8d3be85a5418455fa`.

Evidence: `installed-automated-results.json`, `installed-console-results.json` and the four `docs/screenshots/browser-usability-*.png` captures. Final acceptance finished at `2026-10-22T09:56:28.777Z`. No installed test process remained after cleanup. Delivery branch: `browser-usability-20261022`.
