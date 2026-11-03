# P0 computer/browser and vault

Baseline: `aaaf09fe6c45cce2fe42bf9488ac3815156f4ad9`. This is prerequisite evidence, not a completed Phase 4 implementation or a re-review of owner-accepted Phase 3.

## Checked behavior

- `npx vitest run packages/daemon/src/native-browser-handlers.test.ts packages/desktop/src/native-browser-permissions.test.ts`: **2 files, 4 tests passed**. These are baseline contract/unit checks, not remote browser or vault proof.
- `node scripts/phase4-spikes/computer/run.mjs`: **failed the capture gate** in the hidden disposable Electron fixture. Result: `captureBytes=0`, `nativeCaptureError=Error: UnknownVizError`; CDP screenshot also returned the existing operation-timeout error and `cdpCaptureBytes=0`.
- The same final probe measured `hiddenOwner=true`, `humanBarrier=true`, `resumed=true`. Human control refused a subsequent automation callback, and returning control allowed it again. Its fixture temporarily made child views logically visible without displaying the owner window; that did not establish capture readiness.
- An initial probe hung because it used module-level `await app.whenReady()` in Electron's entry module. The corrected fixture starts an async function without awaiting it at module scope. The later capture failures remain after that fixture correction.
- Disposable profiles, a loopback synthetic page and an explicitly owned child process were used. The runner checks the exact temporary-root parent/prefix before cleanup. No installed app, real browser profile, vault entry, microphone or phone was used.

## Source evidence and missing behavior

`packages/daemon/src/native-browser-handlers.ts` returns `frame: null` and refuses `browser.input`. Resize is state-only on this native path. UI `BrowserPanel` and `nativeBrowserAvailable` require the local native desktop bridge. Existing browser schema/frame code does not establish working native remote control.

`NativeBrowserEngine.#present` hides native child surfaces unless mounted in a visible owner window. The failed hidden-capture probe is a concrete reason to resolve painting/capture behavior before relying on that engine for a tray-hidden host, remote viewport or thumbnails. It is not evidence that capture fails in all environments or visible native views.

Browser control is currently a global human/bot flag, not an authenticated per-device controller lease. `withAutomation` checks it before dispatch, and queued jobs use the engine's tail. That alone does not revoke all in-flight JavaScript, create a private vault state, or establish remote ownership. Mount/hide can currently return control to the bot; a private interaction must override that behavior.

Current `bots.pause` prevents future runner dispatch; it does not establish the accepted mid-turn all-tool pause contract. Keep Pause separate from current Stop/interrupt semantics and wire its enforcement through provider and MCP dispatch gates.

## Shared contracts needed before implementation

The files lane and computer lane need the same trusted request context through `server.ts` and `remote-host.ts`: authenticated connection ID, principal kind/device identity and an abort/close lifecycle. It must originate from the accepted connection, never from client-supplied params. Internal daemon dispatch is a distinct principal and must not automatically gain device-only file/browser/vault capabilities.

Proposed browser lease: bind human control to host/bot/generation, current connection ID and a changing control epoch. Only its current owner may send input, resize, navigate or release. Disconnect, revocation, tab/page changes and host switch invalidate pending input and release held keys/buttons. Local native input and remote input share the same arbiter.

Proposed private state: host-owned private epoch plus explicit begin/end operations. Beginning private state invalidates queued bot work and capture operations; enforce the check before execution and before delivering results. Suppress screenshots/thumbnails and page inspection, evaluation, network/console collection and other output channels for the duration. Private state survives panel hiding/control-client disconnect until the owner explicitly resumes or the host closes the session safely. Never auto-resume because a visual overlay unmounted.

Do not route viewer frames or keyboard text through persistent chat/events. Reuse the existing Noise connection and bounded frame messages; validate page/generation/viewport/lease on every input. Drop stale frames instead of queueing an unbounded video backlog.

## Vault decision and test boundary

The accepted plan's enforceable contract is that Tjommie's vault/fill paths do not emit vault secrets into model inputs, tool results, application screenshots during fill, chat/events, logs or diagnostics. It does not promise that an arbitrary receiving site cannot expose a value later.

OS credential storage protects secrets at rest; it is not a complete security boundary from unrestricted processes running as the same Windows user. In particular, Full auto execution requires a separate containment decision before claiming that a bot cannot read OS-protected credentials through some other process. Do not hide this behind an opaque entry ID or a passing mocked-store test.

Use separately namespaced host credential references and metadata records, reusing transport patterns from `packages/daemon/src/account-credentials.ts` rather than API-key validation rules. No existing real credentials may be enumerated/read by development checks. Native fill must accept an opaque entry reference and exact approved HTTPS origin/profile/page generation, not a bot-provided script or selector.

Secret creation/editing needs a trusted dedicated native/isolated input surface; do not place raw values in the ordinary app store. Remote clients request an opaque host operation and do not receive plaintext. Synthetic-store checks can run without touching Windows Credential Manager; disposable real OS-store acceptance is a later explicit check.

Required tests include queued/in-flight capture races, wrong/stale controller, disconnect with held input, panel hiding during private fill, post-navigation origin changes, unexpected frames, and a unique synthetic-secret absence check across every output sink. A successful secret fill alone is insufficient.

## Implementation ownership proposal

- Protocol: browser lease/epoch/private-state schemas and safe frame/input commands; root owns common protocol wiring/version coordination.
- Desktop: `native-browser.ts`, automation, host bridge and tests for painting, bounded capture, remote input, leases and private-state gates. Dedicated vault store/fill modules and trusted UI bridge follow private-state proof.
- Daemon: native-browser handler/broker/MCP changes, trusted request context, runner/provider pause enforcement and safe vault metadata/native operation routing.
- UI: remote computer pane, visible-card thumbnails, control/connection/private-state presentation and vault metadata/trusted-operation initiation.
- Root integration: main/preload/CSP/shared export and migration wiring, diagnostics exclusions and changelog entries.

**Gate:** remote capture and vault/private-mode acceptance remain open. Resolve capture in a hidden/offscreen source fixture before P7 implementation. No generic capture fallback, automatic control handback or relaxed privacy promise is approved by a failed probe.

## P7 source follow-up — 2026-11-03

The hidden native frame gate is now resolved with the existing native engine. A scoped `Page.startScreencast` makes the hidden native view paint; `Page.captureScreenshot` captures one bounded image, and `Page.stopScreencast` always runs. No owner window or native child is made visible. Offscreen views also painted in disposable experiments, but were not selected because they would change native desktop presentation. The earlier `capturePage` failure remains useful baseline evidence, not the selected production path.

`node scripts/phase4-spikes/computer/run.mjs` passed against Electron **44.4.5**. The final source run captured **4,480 bytes at 640×480 in 176 ms**, with an actual CSS viewport of **640×480**, `hiddenOwner=true` and `fixtureForcesChildVisible=false`. These are fixture measurements, not a performance promise. The fixture verifies synthetic PIN insertion, character keys, Ctrl+A, wrong/stale controller refusal, held Shift/mouse release on disconnect, private-state persistence on hiding/disconnect, explicit safe resume, and absence of a unique synthetic value from checked state, notice, private-tool and private-frame outputs. It deliberately does not enumerate or use a credential store. `remoteTransportTested=false` and `vaultFillTested=false` remain explicit in its output.

Production source now routes remote frame/input/control through the authenticated request context and private native-host bridge. The host binds a single controller to the server-created connection ID and validates generation, active page, viewport and control epoch. Disconnect/revocation uses an urgent broker lane and invalidates late replies. Native local control uses the same arbiter. Privacy epochs fence queued and in-flight automation/capture results; private state suppresses screenshots, state metadata, console/network observation and tool outputs, and survives panel hiding or controller disconnect. The trusted host must verify fields are cleared/submitted before ending private mode; ending privacy alone does not resume automation.

The remote Computer pane reuses the existing frame viewport, tabs and controls. Android can use an explicit text-entry field for its soft keyboard. Browser-enabled agent cards and the Computer card show a visibility/foreground/connection-gated preview, with one outstanding request, a one-second interval, a 320×240/64 KiB image limit, and no persistent image cache. Native desktop mounting, persistent profiles and direct input remain in place. Actual visible desktop/native presentation and physical Android behavior have not been tested by this hidden fixture.

Focused verification: **15 files / 125 tests passed**, including privacy/controller helpers, bounded frame lifecycle, automation, private host/broker/MCP, protocol, BrowserSession/preview and UI callers. The production browser RPC integration fixture was adapted to the new host response, and its focused test passed. The root coordinator owns the protocol-version change and combined suite/typecheck results.

## P8 remains blocked by containment and trusted secret input

No production vault store or fill API is exposed by this lane. OS credential protection does not isolate a secret from unrestricted Full auto processes running as the same Windows user. An opaque entry reference, private browser epoch or mocked credential store cannot fix that boundary. Enabling vault fill under that condition would make the accepted disclosure claim false; the safe result is to remain unavailable until a real process/user isolation boundary and a trusted native secret-entry surface are implemented and verified.

The private-mode foundation and synthetic output checks above are prerequisites only. They are not exact-origin HTTPS vault fill, OS-store acceptance, complete prompt/log/diagnostic sink acceptance, restart safety, physical Android acceptance, or proof against a receiving website re-exposing a submitted value. No owner credentials, installed application/profile, phone, microphone, paid provider, cloud deployment, build, commit or push was used.
