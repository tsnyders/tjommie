# Phase 1 follow-up: embedded browser and quiet startup

Date: 2026-10-22. Worktree: `tjommie-phase1-completion`, branch `phase1-completion-20261021`, based on `a6d2238`.

Status: source integration passed; implementation committed as `6311819`. The refreshed installer is installed and all automated acceptance checks passed. The owner deferred the usual-site embedded login check until later. The previously installed Phase 1 artifact does not establish acceptance of these follow-up changes.

## Changes

- One daemon-owned persistent browser context per bot. The interactive application side panel and bundled Playwright MCP share pages, tabs and signed-in sites. Existing `chrome-profile` directories are retained. The engine is headless Chrome/Edge with a private process pipe, not a separate visible browser window or an exposed browser-debugging port.
- Explicit Take control / Resume bot, navigation, back/forward/reload, tabs, JavaScript dialogs, pointer/keyboard/wheel/paste input and a keyboard exit route. Closing the panel releases input and returns control to the bot without closing its browser. Same-bot reopen waits for the previous panel's teardown.
- Frame/page/generation checks and invalidation around structural navigation/resize changes prevent old page pictures from sending input into changed layouts. Browser cleanup attempts independent resources even after a failure. Download saving has one owner while MCP is connected and returns to the manager after disconnect.
- Chrome/Edge discovery checks machine, x86 and per-user installations and reports a missing engine explicitly.
- Codex quota selection considers primary and secondary windows, preserves percentage/reset pairs, and waits for the later reset when both are exhausted.
- Routine failures persist in an outbox atomically with failed history. The desktop subscribes, fetches pending notices, deduplicates live/replayed run ids, and acknowledges only after Windows confirms showing a notification or the foreground suppression policy handles it. Failed/unsupported display stays pending.
- Windows Codex npm installations resolve their bundled native executable directly. The npm wrapper was independently proven to create a visible terminal despite its parent being hidden. Native launches retain managed-package metadata, the helper path and account isolation. Unknown layouts retain the compatible wrapper fallback. Forced daemon termination helpers are hidden too.
- Compatible dependency patches were applied. Production `npm audit --omit=dev` reports zero vulnerabilities. The full audit retains 13 affected development-tool entries (5 high, 8 moderate) rooted in `braces` and `sprintf-js`, without compatible patched releases; no forced downgrades were applied.

## Verification record

- Final combined fast suite: **937 tests / 73 files passed**.
- Root/UI TypeScript, syntax-based no-`any` gate (**214 TypeScript files**) and UI lint passed.
- Production Next 16.3.8 static build passed.
- Final real-browser integration: **2 tests / 2 files passed**, covering the daemon's shared context and the actual desktop side panel. The tests include synthetic sign-in persistence, two-bot isolation, dialogs, downloads, MCP disconnect survival, popup resize recovery, exact JPEG dimensions and click alignment.
- Independent read-only reviewers' findings were fixed and re-reviewed.
- `npm run package` passed. Mandatory `npm run test:packaged` passed: **1 executed test, zero skips**, with current input and all artifact hashes verified before and after execution.
- Per-user NSIS installation exited **0**. Installed executable and every payload hash matched the verified build before normal-root acceptance.
- Exact installed browser panel: **1 real test passed** (16.87 s), including mouse Go, typing, dialogs, popup close/resize, handback, reopening, isolation and owned headless/pipe/no-TCP/no-window checks.
- Normal-app Own folder acceptance passed using the owner-authorized existing managed Codex account: in-folder work without approvals, same bot/file and another successful turn after restart, fixture deletion, and **zero new visible terminal windows** at 50 ms sampling. See [own-folder-results.json](own-folder-results.json).
- Installed startup routine failure replay passed: orphan history is failed durably, and the desktop receives/handles/acknowledges the notice after daemon restart. This was intentional foreground suppression, not an assertion of OS toast rendering. See [startup-notice-results.json](startup-notice-results.json).
- **All 9 installed Phase 1 acceptance checks passed**: provider readiness, cross-provider exchange, original-turn steering, both Stop timings, persistent exact Write approval, browser tools/profile, approved routine proposal, and scheduled execution with the window closed. Stop took **594 ms** immediately after approval and **602 ms** after the sandbox command was running; both persisted as interrupted. See [installed-automated-results.json](installed-automated-results.json).
- Final provenance verification passed again after acceptance. Normal data retained **2 bots / 1 enabled routine**, and no temporary Own folder bot remains. The app was left closed.
- Screenshots: [embedded side panel](../../screenshots/phase1-embedded-browser.png) and [installed chat](../../screenshots/phase1-installed-main-chat.png). The local synthetic page is intentionally plain; application chrome uses the existing wireframe/design system with the owner-requested browser extension.

## Limits

- Manual native file pickers, operating-system authentication dialogs and passkey flows are not exposed by this panel. Attach files in chat and ask the bot to upload them. A pending file chooser can be cancelled by the bot with an empty file list after Resume bot; the actual cancellation recovery is covered by the synthetic browser test.
- Synthetic sites establish cookies/localStorage persistence, isolation and shared bot/manual sessions. They do not establish compatibility with every third-party login provider. The owner chose to test a usual site later on October 22; that manual acceptance remains pending for this new embedded implementation, and the earlier external-browser confirmation does not substitute for it. Existing real credentials are never copied to make a fixture pass.
- A crash after a Windows notification is shown but before its acknowledgement can replay that notification after restart. Foreground suppression and actual OS toast rendering are distinct acceptance cases.
- Normal startup and bot work should remain silent. Explicit Claude account sign-in still uses the provider's interactive sign-in console; this change does not introduce an embedded provider terminal.
- Unknown/localized/partial Windows process termination reports still fail closed. Existing process-tree enumeration limits from the Phase 1 acceptance report remain; this is not Windows Job Object containment.
- Current Chrome supports the experimental native contents-size repair. Older Chrome/Edge are not separately verified; an explicit unsupported-method response falls back to Playwright viewport sizing, and mismatched captures stay hidden.
- One earlier synthetic browser test directory could not be removed because tool policy blocked deletion. Its owned browser processes were stopped. Later disposable browser tests cleaned up their own roots.

Nothing is pushed, published or merged into `main` by this task.

## Artifact

- Implementation commit: `6311819`.
- Installer: `dist-desktop/installer/Tjommie Setup 0.1.0.exe`, 120404614 bytes, unsigned per-user Windows x64 build.
- Installer SHA-256: `e95164955a8ec356644ff316222f62b0c51ae447f4e8541c4d041e3b5846643b`.
- Build input SHA-256: `e4305abb6540bfa2de41481982db731e2af7e06a0238e7c7da8aaee7cfea20eb`.
- Provenance covers **569 artifact files**.
