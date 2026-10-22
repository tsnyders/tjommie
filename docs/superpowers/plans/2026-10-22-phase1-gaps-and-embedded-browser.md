# Phase 1 gaps and embedded browser

User authorization (2026-10-22): finish the remaining Phase 1 gaps and replace the external bot browser window with an interactive browser built into the application's side panel. Use Paseo agents, including parallel agents with separate ownership. Never use `any` as a TypeScript type.

Worktree: `tjommie-phase1-completion`, branch `phase1-completion-20261021`, starting at `a6d2238`. Preserve the original checkout and unrelated work. No push or publication is included.

## Scope and order

- [x] Investigate and specify a shared browser session for bot automation and interactive side-panel browsing, including authentication, profile persistence, existing profile compatibility, popups, and cleanup.
- [x] Implement embedded browsing: open/close panel, navigation, back/forward/reload, page input, loading/error states, bot selection isolation, and persistent sign-ins. Closing the panel must not prevent bot automation or lock its profile.
- [x] Verify browser discovery across Chrome and Edge install locations, with a useful missing-browser error.
- [x] Represent secondary Codex quota windows without dropping a blocking limit or using an incorrect reset time.
- [x] Recover startup routine-failure notices so they reach the desktop after startup without repeated notifications on reconnect.
- [x] Triage current dependency advisories and apply scoped compatible remediation with evidence.
- [ ] Verify normal persistent-root Codex Own folder behavior without changing personal CLI configuration or account credentials.
- [x] Eliminate terminal windows during normal app launch and bot use (additional owner request). Trace nested CLI launches as well as the daemon; verify owned window/process behavior, not just spawn options.
- [x] Obtain independent read-only review, fix findings, and run combined validation.
- [ ] Rebuild and verify the package, exercise the embedded browser in the exact app artifact, and record results and remaining limits.

## Ownership and integration

The coordinator owns this plan, integration, documentation, combined checks and package delivery. Specialists receive explicit file lanes before editing. Shared protocol and daemon entry-point changes are coordinated before work begins. Reviewers report findings only and never edit files. Dependency changes are serialized before integration checks.

## Acceptance

The bot browser appears inside the side panel, with no separate browser window for normal bot browsing. Manual navigation and bot automation see the same pages and cookies. Test keyboard, pointer, scrolling, page switching, navigation failures, persistent cookies across restart, two-bot isolation, hide/show behavior and cleanup. Test authentication and input validation at the browser-control boundary; do not expose an unrestricted browser debugging endpoint to application pages.

Use disposable test profiles and local synthetic pages for automated login/cookie checks. Real sites requiring credentials remain owner acceptance; never read or copy credentials to fabricate that result. Preserve existing profile data and document compatibility limits explicitly.

Run TypeScript, the syntax-based no-any gate, lint, relevant regression tests and production build. Package verification must validate current input/payload provenance and actually execute its required smoke test. Source tests alone do not establish installed acceptance.

This request supersedes the old Phase 1 external "Show window" browser interaction. It brings shared interactive browser presentation forward from the later computer-view roadmap; it does not add the credentials vault, phone access or general desktop control.
