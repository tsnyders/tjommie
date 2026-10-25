# Phase 2 Accounts and Usage, then Phase 2.5 Phone UI

Authorized by the owner on 2026-10-22. Base: main `bea2b64`; implementation branch: `phase2-accounts-mobile-20261022`.

## Delivery order

1. Freeze account, usage and provider-runtime contracts after reading the existing implementation and current provider interfaces.
2. Migrate legacy accounts without moving credentials or changing existing sign-ins. Add account management, per-bot selection, provider-native sign-in and protected API-key storage.
3. Attribute usage to the actual running account. Add limit status, explicit switch/wait, opt-in fallback and spending-cap handling with honest unknown-cost states.
4. Implement Accounts and Usage screens and integrate account selection into bot forms, setup and limit cards.
5. Complete independent read-only reviews and Phase 2 runtime/migration checks before extending layouts.
6. Implement Phase 2.5 responsive navigation and every screen at phone widths. Remote connectivity, Android packaging, pairing and push remain Phase 3.
7. Run combined checks, real desktop and narrow-screen acceptance, package provenance and installed validation. Keep the current installed working version until a replacement passes.

## Invariants

- Never use `any` as a TypeScript type. Reviewers report findings without editing files.
- Existing provider credential folders and IDs survive migration. Agents and verification must never inspect owner credential files, login tokens or passwords. The later Paseo-style Usage request permits only the narrowly scoped production Claude usage reader described in spec §10.2; no broader token copying, persistence, refresh or exposure is added.
- API keys are entered through password fields and stored in Windows Credential Manager; only secret references enter SQLite. Keys never appear in status, events, argv, chat or logs.
- Provider-native sign-in is an explicit user action. No unsolicited sign-in windows, terminal windows, credential changes or billable provider calls during synthetic verification.
- Each bot is pinned to a provider-compatible account. Effective fallback identity is captured with the running session and usage events; it cannot be inferred from a later mutable default.
- Switching accounts starts a fresh provider session with a recap and preserves the chat. No stale async response may restore an old account or sign-in attempt.
- Local spending controls cannot guarantee the provider's final bill. Unknown or unsupported pricing must never be shown as zero or silently bypass a configured cap. Subscription API value and API-key spending are labeled separately.
- No account or bot deletion removes a user project folder. Historical usage remains attributable after account removal.
- The native browser remains per-bot and desktop-owned. A phone-sized desktop window may display it; an actual web/phone client must show the honest desktop-required state until Phase 3 provides a supported transport.
- Do not change the original dirty checkout, stop shared services, alter existing accounts, or merge/push this follow-up without later authorization.

## Agent selection and ownership

Profiles and all notes were read from Paseo before selection. Design Lead owns the initial screen/phone design proposal. Lead & Build investigates account storage and usage queries. Deep Investigation owns provider-runtime and credential isolation analysis. UI Builder will implement settled screen contracts. Independent Claude/Codex reviewers remain read-only.

The coordinator owns protocol contracts, bot/daemon integration, pricing, acceptance, cross-lane coordination and this plan. Storage owns the append-only migration, account store and usage queries. Runtime owns account preparation, provider sessions, credentials, parsers, CLI environment and account ACL integration. UI Builder owns Accounts, Usage, bot selection and banners; phone layouts follow the Phase 2 gate. Reviews remain read-only.

## Settled implementation choices

- Account caps cover lifetime recorded API spending. They never reset silently. Unknown cost blocks later API work; supported Codex model pricing is checked before starting a paid turn. Provider-reported Claude cost is preferred, and missing cost remains unknown.
- Sessions use `(bot_id, provider, account_id)` as their identity. Usage captures the actual account at session creation, including fallback, rather than looking up a later default.
- Reached quota windows block even with an unknown reset. The optional reset timestamp is not a readiness flag.
- Unknown-reset recovery is explicit: Codex subscription status can read authoritative quota metadata without a paid turn. Separately, Retry queued work grants one expiring attempt on the selected account, under its normal auth, pricing and budget gates. A failed or stopped retry keeps the old block; a newer quota event cannot be erased by an older probe or retry result.
- Windows Credential Manager is the persistent API-key store. Codex API authentication is supplied through its owned app-server pipe without creating a second persistent key copy. Claude API authentication enables subprocess environment scrubbing.
- Backend sign-in owns the initial system-browser launch. The UI keeps a returned URL only for an explicit reopen action. Claude sign-in must remain hidden and use loopback completion; unsupported manual-code fallback must not open a console.
- Explicit account removal disconnects only that account, preserves history and folder metadata, and fails without tombstoning if local disconnect fails. Migration never signs accounts out.
- The original spec names OpenPencil for phone designs. That integration is unavailable in this session; a self-contained local HTML visual specification and Markdown handoff will provide the reviewable design artifact before narrow-screen implementation. Pairing and Devices may appear as clearly labeled future designs, without implementing Phase 3 transport.

## Integration evidence so far

- Synthetic daemon checks cover captured account attribution, stable default pins, API-key result redaction/removal, failed subscription logout followed by retry, and cap blocking/resume after an explicit increase.
- A real isolated source-desktop check passed account rename/default changes, existing-bot repinning and separate API-spend/subscription-value totals. It made no paid turn or sign-in.
- Independent UI review found unknown-reset, stale-dialog, stale-usage and refresh starvation issues; corrections and re-review remain required before the phone layout lane starts.
- Those five UI source findings were corrected and cleared in narrow re-review. Subsequent desktop captures verified the dated daily chart and time-zone caption. The first combined suite passed 1,310 tests and found two account-override ACL integration failures; both focused regressions passed after consistent accounts-root plumbing, and the ACL trust-root correction cleared read-only review.
- The phone visual specification is `docs/design/phase2-5-phone.html` with its Markdown handoff. All 15 frames were rendered at 390×844 and 360×640 in both themes without horizontal phone-frame overflow. The coordinator inspected the Agents, chat/approval, Details, browser slot, new-bot picker, Accounts, key dialog and Usage captures. This verifies the design artifact, not the implemented phone UI.
- Phase 2 engineering gate cleared: 1,350 tests across 107 files passed with four workers, root/UI typechecks, no-any and lint passed, and both independent source reviews cleared the corrected implementation. An earlier unrestricted concurrent run timed out several subprocess startup fixtures; the bounded full rerun retained all assertions and timeouts. Actual owner sign-in and paid-provider acceptance were not performed.
- Phase 2.5 implementation is authorized in the UI lane against that visual spec. The coordinator lowers the desktop window minimum width to 360 and owns real narrow-screen acceptance; connectivity and Android work remain excluded.

## Current delivery scope

The owner requested a testable Phase 2 build before continuing Phase 2.5. Phone implementation is paused. The UI lane confirmed no phone edits were applied; the coordinator restored the desktop minimum width to 960. The reviewed phone design files remain for later work. The current package contains the completed desktop Accounts and Usage changes only.

The owner subsequently authorized installing that verified Phase 2 build and resuming mobile layout work (called Phase 2.1 in the request, confirmed to mean the earlier Phase 2.5). The per-user installer exited 0; all 567 installed payload files matched the packaged files, and the installed executable passed the isolated Accounts real-app test. Mobile work now starts with three interactive design choices in `docs/design/mobile-pill-options.html`, all with floating pill navigation. The responsive production design awaits the owner’s choice. No owner sign-in, logout or paid provider turn was performed during installation verification.

The owner selected Quiet pill. UI-B is authorized against that direction and the F1–F13 handoff, with floating labeled pill navigation replacing the original flush bottom bar. The coordinator restores the planned 360 px desktop minimum and owns synthetic real-DOM acceptance. Production phone connectivity, pairing, Android and real provider operations remain excluded.

The owner also requested Paseo-style Usage provider cards and a combined token cost estimate for today, week, month and year. This explicitly adds an API-equivalent combined estimate to the existing separate spend/value accounting; it must not be presented as the subscription bill. Year means the current calendar year to date, respecting report time zone. Provider cards show only metadata actually supplied by the provider; missing quota/credit/extra-usage data stays unavailable. Free metadata refresh and source inspection are in scope; tests do not use owner credentials or send paid turns.

The public account payload adds optional `providerUsage` metadata for compatibility with older snapshots. The runtime supplies availability/error/freshness, display windows, plan, provider credit units and an optional extra-usage boolean. This stays separate from stored account-wide quota gates and historical token costs. Codex uses its existing app-server metadata RPC; Claude's production reader is limited to the selected account folder and a fixed OAuth usage endpoint, following the inspected Paseo implementation. Refresh failure retains the last successful display data, and null/unknown allowance permission must not erase a known block.

## Acceptance

- Legacy database migration preserves both existing accounts, bot sessions and recorded usage.
- Multiple accounts, per-bot pinning and account switching work without cross-account credential leakage.
- Synthetic provider streams prove account-level limit handling, fallback, reset/resume and caps; no real limit or paid call is manufactured.
- Usage filters and totals handle missing cost, reported/estimated cost, time zones, empty states and deleted entities correctly.
- Secrets remain absent from snapshots, logs, rendered markup, events and errors; Windows secret-store round trips use synthetic values only.
- All screens work at 360 and 390 px widths, with accessible navigation, focus, dialogs, approvals and no horizontal page overflow.
- Desktop browser takeover, resize, popups, stored sign-ins and isolation retain their existing real-test coverage.
- Typechecks, no-any, appropriate tests, lint, build, independent reviews and installed acceptance pass before delivery is called complete.
