# Native browser migration

The owner approved the Paseo-style desktop-owned native browser on 2026-10-22. This supersedes JPEG streaming as the human-facing desktop browser. Preserve the completed Details/Browser shell, splitter, expansion, caption spacing and rail padding.

## Ownership

- Native engine agent: desktop native-browser modules, protocol native-browser command schemas, disposable Electron engine tests.
- Automation agent: desktop native-browser-automation and daemon native-browser-mcp modules/tests.
- UI agent: native browser session and viewport integration, native DesktopApi declarations. Google API card removed from active bot details while the separate API work is on hold.
- Coordinator: private daemon/desktop bridge, process credentials, main/preload wiring, lifecycle integration, combined tests, documentation, package and installed acceptance.
- Reviewers report findings without editing files.

## Invariants

Each bot receives a separate persistent Electron session below the protected application data directory. Old Chrome profiles remain untouched; there is no cookie/profile import. Human and bot operations target the same live WebContents. Website guests have no application preload, Node integration or generic application IPC access. No external debugging TCP listener is opened.

The desktop host receives a separate random capability over its private startup pipe. It is never returned to the renderer or bot CLI. Native browser tool requests use a bounded private host route, not public renderer broadcasts. The main process validates the app sender and resolves the bot's actual stored identity before mounting or controlling a guest.

Takeover remains explicit. Hiding the pane returns control without destroying the session. App dialogs and hidden windows occlude guest surfaces. Disabling/deleting a bot tears down its native guest resources. Desktop residency is required for bot browser tools.

Google website sign-in is unverified. Paseo source demonstrates native rendering and persistent sessions, not a Google authentication workaround. OAuth API work remains separate and on hold. No real Google credentials or account changes are part of synthetic validation.

## Acceptance

- Native human typing/clicking, audio/video rendering, resize, expand/restore, tab persistence and per-bot isolation.
- Same-tab agent navigation, snapshot/ref actions, forms, dialogs, uploads/downloads and popup semantics.
- No old JPEG streaming or hidden Playwright browser in the production desktop path.
- Strict owner-bound IPC, private-host capability checks, teardown and unavailable-desktop errors.
- Root/UI typechecks, no-any, focused and combined suites, independent read-only reviews.
- Fresh source desktop tests, package provenance and installed acceptance before claiming delivery.
- Owner-controlled Google website sign-in test remains a separate compatibility gate.

No push or merge is included in this follow-up. Never use TypeScript any types.
