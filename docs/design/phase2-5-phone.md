# Phase 2.5 phone screens: design spec and UI-B handoff

- **Date:** 2026-10-22. **Branch:** `phase2-accounts-mobile-20261022`.
- **Visual reference:** `docs/design/phase2-5-phone.html`, a self-contained contact sheet that needs no network. It has frames F1 to F15 at 390 × 844 or 360 × 640, dark or light, with the Windows caption buttons shown or hidden.
- **Why HTML, not OpenPencil:** spec 9.1 asks for phone designs in OpenPencil before building. The OpenPencil integration wasn't available in this session, so the design is recorded as HTML built from the app's own tokens (`packages/ui/app/globals.css`), lucide icons (paths from `lucide-react` 1.48.0), bot avatars (`components/bot-avatar.tsx`) and the shipped copy. OpenPencil frames can be drawn from it later. Until then, this file and the HTML are what "matches its design" means for Phase 2.5.
- **Scope:** responsive layout of every existing screen, including Phase 2's Accounts and Usage. Relay, pairing, devices, the Android app, push, safe areas, the soft keyboard and device-code sign-in are Phase 3. F14 and F15 are labeled design-only and must not be built.
- **Sample data:** bot names and numbers are examples. Email addresses use `you@example.com`.
- **Owner selection (2026-10-22):** Quiet pill from `mobile-pill-options.html` supersedes the original F1 bottom-bar treatment. Use a floating fully rounded pill with three labeled destinations, an inset of about 18 px, a bottom gap of about 20 px, token-colored card background and border, and a selected capsule. Reserve space below page content so the pill covers no controls. The prototype's sample activity and summary are not product data and must not be copied into the app.

## 1. Frames

| Frame | Screen | Route or entry | Matrix |
|---|---|---|---|
| F1 | Agents (phone home) | `/agents/`; `/` on phones | M-01, M-02, M-03 |
| F2 | Chat with an approval | `/chat/?bot=` | M-04 |
| F3 | Details sheet | The title button in F2 | M-05 |
| F4 | Browser in the narrow desktop window | The Browser tab in F3 | M-06 |
| F5 | Browser on a client without the desktop app | The Details sheet | M-07 |
| F6 | Team room with the mention popover | `/room/` | M-09 |
| F7 | Exchange view | A handoff row or Open exchange | M-08 |
| F8 | New bot with the account picker open | `/bots/new/` (Bot settings is the same) | M-10 |
| F9 | Accounts: a sign-in waiting in the browser, and a Codex subscription limited with an unknown reset | `/settings/?section=accounts` | M-11, M-16 |
| F10 | Add API key (full-screen dialog) | Add → API key | M-11 |
| F11 | Add subscription (full-screen dialog) | Add → Subscription | M-11 |
| F12 | Usage | `/settings/?section=usage` | M-11 |
| F13 | Settings list | `/settings/` on phones | M-11 |
| F14 | Pair a device (**Phase 3, design only**) | none in 2.5 | none |
| F15 | Devices (**Phase 3, design only**) | none in 2.5 | none |

## 2. Global rules

- **Breakpoint:** phone means a viewport narrower than 640 px (Tailwind `sm`). Layout changes use `max-sm:` and `sm:` classes. JavaScript only where behavior changes: `isPhone(room)` with `PHONE_BELOW = 640` in `lib/panel-layout.ts`, and `matchMedia("(max-width: 639px)")` for the `/` redirect. Widths of 640–999 keep today's overlay panel; 1000 and up is unchanged.
- **Caption buttons:** in the desktop window, Windows draws its caption buttons over the top-right 138 × 44 px at every width. Every phone header and sheet bar keeps the existing 146 px right clearance (`pr-[146px]`, `CAPTION_CLEAR`). Clients without the desktop app (F5, Phase 3) use 16 px.
- **Tokens and type:** token classes only, no new colors. Type sizes are 12, 13, 14, 15, 17 and 20, with 11 for chart ticks and captions. Weights 400, 500 and 600. Radii as in the wireframe spec: cards 12, large cards 14, popovers and dialogs 16, rows 10, popover rows 8, pills fully round.
- **Targets:** at least 32 px everywhere. The tab bar, the Back link and the chat title button are 40–48 px. Focus rings are unchanged: 2 px accent, outside buttons and inside inputs.
- **Overflow:** no horizontal page scroll at 360 px on any screen. Long names truncate and their trailing chips, counts and chevrons stay visible. The browser tab strip scrolls sideways inside itself.
- **Motion:** sheets slide up over 200 ms with the existing easing. The global reduced-motion rule turns this off.
- **Shell:** `h-screen` becomes `h-dvh`. The desktop window's `minWidth` drops from 960 to 360 (`packages/desktop/src/main.ts:401`, coordinator-owned) so this layout can be used and tested in the real window.

## 3. Navigation

- **Tab bar** (`components/tab-bar.tsx`, `sm:hidden`, `nav aria-label="Tjommie"`; the rail becomes `max-sm:hidden`):
  - Tabs are **Agents** (`LayoutGrid`, `/agents/`), **Team room** (`Users`, `/room/`) and **Settings** (`Settings`, `/settings/`). The label is "Agents", matching the rail, which supersedes "Bots" in the earlier handoff.
  - Quiet pill: approximately 64 px high including internal padding, rounded-full `bg-card` with a token-colored border, horizontally inset about 18 px and floating about 20 px above the bottom. Selected tabs use a rounded `bg-selected` capsule and carry `aria-current="page"`. All three icons have visible text labels. The layout reserves the pill's height and gaps instead of drawing it over interactive content.
  - When any bot is waiting, the Agents tab shows a 10 px accent dot, and its name becomes "Agents, N waiting for you".
  - It shows on `/`, `/agents/`, `/room/` and `/settings/*`. It's hidden on `/chat/`, `/bots/new/` and `/bots/settings/`, which have a composer or action footer.
- **Back** (`components/back-link.tsx`, `sm:hidden`): a `Link` with a chevron.
  - It's named "Back to Agents" in chat, "Back" in New bot and Bot settings (to the previous page, falling back to Agents), and shows visible text "Settings" in Settings sections.
- **`/`:** on phones, a user with bots is sent to `/agents/` with `router.replace`. On desktop it still opens the first chat. First run is unchanged apart from `px-4 pt-6`.
- **Switching bots** on phones means going back to Agents; there is no rail.

## 4. Screens

| Frame | Layout at phone width | Focus when it opens | Focus when it closes |
|---|---|---|---|
| F1 Agents | Header: "Agents", Search, + (New bot). The summary and filter `Segmented` sit on their own rows. Cards become rows: 28 px avatar, name, state chip, task line, and an always-visible ⋯. No thumbnail, next-routine line or footer. Team room and New bot cards are hidden. The waiting row keeps the `waiting-stroke` border. | The first row link | – |
| F2 Chat | Header: Back, a **title button** (24 px avatar, name 15/600, state 12 with the existing color rules), More. On phones the title button replaces the panel icon and opens F3, with `aria-expanded` and the name "{bot}, {state}. Show details". Messages `px-4`. Bubbles full width; user bubbles at most 85%. Approval and limit buttons wrap. Composer full width. The panel starts **closed** on phones, and phone toggles never write `tjommie.panel`. | The composer | – |
| F3 Details | Full-screen `<aside>` over the chat. The bar has Close (X, "Close details") and the Details/Browser tablist. The gear moves to the More menu. No separator. Content is the existing bot panel. | The selected tab | The title button. Escape closes it unless the target is `[data-keeps-escape]` or a page dialog is open. |
| F4 Browser (desktop app present) | The existing native pane, at full sheet width. The toolbar wraps: a tab strip that scrolls sideways; Back, Forward, Reload and the address field; the control line with Take control or Resume bot. The slot is at least 240 px tall. Expand is hidden; entering phone width clears `expanded`. The page hides under any app menu, list or dialog (existing `APP_OVERLAY`). | Take control | Same as F3 |
| F5 Browser (no desktop app) | When `desktop()?.nativeBrowser` is missing, there is no Browser tab and no Open link. The Browser card shows a monitor icon and "Runs on your PC. Open Tjommie on that PC to watch {bot} or take over." The More menu has no "Open browser". Waiting cards replace Take over with the text "Take over on your PC". This replaces `browser-panel.tsx:37`. | Close | – |
| F6 Team room | Header: the Users icon with "Team room" and "6 bots", plus the members button. Members open as a full-screen sheet like F3. Posts fit at full width. The mention popover sits 16 px from the left and 8 px above the composer, `w-[min(280px,calc(100vw-32px))]`. The tab bar shows. | The composer | – |
| F7 Exchange | Full-screen sheet like F3. The header shows the two avatars, "{a} asked {b}" and the meta line. The composer has both bots as chips, and the placeholder truncates. | Close | The handoff row or Open exchange link that opened it (existing) |
| F8 New bot and Bot settings | Back and the title. The form drops `min-w-[532px]`, `w-[640px]` and the 120 px left padding. The avatar tile is centred, and swatches and shapes wrap. Suggestions or About go under the form (existing wrapping). Picker popovers are at most 320 px and stay inside the viewport. The footer wraps to two lines when it must (Bot settings has Delete, Pause, Cancel, Save). The routine editor is full screen (§5). | Name | – |
| F9 Accounts | Back ("Settings"). Group headers fit on one line. Cards stack full width. Status actions wrap under the status text; the retry caption sits under the actions (§7.1). Everything else is UI-A as built. | Back, or the button a link points to (`action=signin`, `cap` or `retry` only focuses it) | – |
| F10, F11 Account dialogs | Full screen (§5). | Name, or the API key field | The control that opened it |
| F12 Usage | Range `Segmented`. Tiles in an auto grid (`minmax(150px,1fr)`, two to a row at 360). Bars card. Breakdown header wraps if needed; rows as in UI-A. | Back | – |
| F13 Settings list | With no `section`, the nav fills the screen as full-width rows: General, Bots, Accounts, Usage, each with a chevron. A section shows "Back to Settings". The General link becomes `?section=general`. Desktop still treats no section as General. | General | – |

## 5. Sheets and dialogs

- **Sheets (F3, F4, F7 and members) stay `<aside>`.** `role="dialog"` would trip `APP_OVERLAY` and hide the native browser inside the sheet.
  - Use the existing `coversChat` to make the chat inert, and the existing `focusPanel`, `hidePanel` and `closeExchange` paths for focus.
  - On phones the panel is `absolute inset-0 z-20` with no width, separator or shadow.
- **Dialogs 420 px or wider become full screen on phones.** That's the routine editor, the account dialogs and Google.
  - Below `sm` they are `inset-0` with radius 0. A 44 px bar holds Close at the **left**, clear of the dimmed caption buttons. The title is the first line of the content. Actions sit in a footer with a divider.
  - `Dialog` gets this behavior through classes, not the inline width (move the width into a CSS variable).
  - 320 px confirm dialogs stay centred cards; `max-w-[calc(100vw-48px)]` already gives 312 px at 360.
  - The caption-button dimming (`setDialogScrim`) is unchanged.

## 6. Native browser rule

Decide by **capability, not width**.
- If the desktop app's bridge exposes `nativeBrowser`, the browser works at any width, including a phone-sized desktop window (F4).
- If there's no bridge, which means a real web or phone client, show the honest state in F5. That holds until Phase 3 provides a supported transport.
- Never show a stream, a screenshot or a pretend page. The browser's state persists on the desktop only.

## 7. Account sign-in

- **No terminal or console screen exists in any frame, and none may be built.**
- Subscription sign-in: "Add and sign in" (F11) or **Sign in** on a card. The daemon opens the provider's page in the **system browser**, never a bot's browser profile.
- While waiting, the card shows "Finish signing in in your browser", **Open the page again** (only when the URL is known) and **Cancel** (F9).
- Links never start a sign-in.

### 7.1 Limited, reset time unknown

This is implemented Phase 2 UI, shown on F9's Codex card. Nothing retries by itself.
- **Status:** an hourglass and "Limited. Reset time unknown." The account **stays limited** until a turn succeeds or a reset time arrives.
- **Retry queued work** (primary pill, calls `accounts.retryLimit {id}`):
  - It shows only when `canRetryLimit` is true: the reset is unknown, no known future reset applies, and the key isn't blocked by its cap.
  - Under the actions, always, the caption: "Retries one queued turn; it may use subscription allowance or API credit."
  - A tap lets one queued turn run. It's never automatic. The daemon still checks the cap, prices and sign-in.
- **Check again** (secondary pill, calls `accounts.check {id}`):
  - Codex subscriptions only. It's Codex's own free quota check: it only looks and runs no work.
  - It can sit beside Retry queued work. The two are different actions.
- **Phone layout:** the status text keeps at least 180 px. The two pills wrap onto the next line together (Retry first). The caption follows on its own line at 12/18 `text-secondary`.
- **Chat banner:** a limited bot with no known reset shows **Manage account**, which links to `?section=accounts&account=<id>&action=retry`. The link **only focuses** Retry queued work, like the `signin` and `cap` links; it never runs it.
- **Focus:** arriving by that link puts focus on Retry queued work (`data-account-action="retry"`). Tab moves on to Check again. A failure appears as the card's `role="alert"` line under the status, and focus stays on the pill.

## 8. Phase 3 frames (F14, F15)

- They show spec 9.2's intended Pair a device and Devices screens, so Phase 3 has a starting point. **Nothing in them exists.**
- They aren't in the Settings list. The QR area is an empty placeholder, and the phone row is labeled a sample.
- UI-B must not build routes, components or copy for them.

## 9. Copy fixes found while drawing

- **`lib/usage.ts` cost lines:** "Spent At least $2.10" should read "Spent at least $2.10". Lowercase "at least" when a label comes before it (Usage breakdown rows and `botDayText`). F12 shows the fixed copy.
- **Earlier handoff wording:** the tab is "Agents", not "Bots". On phones the chat title opens Details instead of a separate panel icon. Dialog Close sits at the left on phones.

## 10. Acceptance matrix (Phase 2.5)

**Sizes:** 390 × 844 and 360 × 640, plus 1440 × 900 to check desktop is unchanged.

**Global checks on every screen:**
- No horizontal page scroll: `document.scrollingElement.scrollWidth <= innerWidth`.
- Nothing under the top-right 146 × 44 in the desktop window.
- Text truncates rather than overlapping.
- Everything works by keyboard and focus is visible.
- Targets meet §2.

| ID | Pass condition | Frame | Verified by |
|---|---|---|---|
| M-01 | The tab bar matches §3: tabs, `aria-current`, the waiting count in its name, visibility by route, named Back links. | F1, F6, F9, F12, F13 | Component tests and screenshots |
| M-02 | `/` sends phones to `/agents/` and desktop to the first chat. | F1 | Unit test |
| M-03 | Agents rows as in §4. | F1 | Screenshot |
| M-04 | Chat: title button with `aria-expanded`, panel closed by default, `tjommie.panel` untouched, approval and limit buttons wrap. | F2 | Component test and screenshot |
| M-05 | Details sheet: full-screen aside, chat inert, focus moves in and back, Escape works, no separator or gear. | F3 | Component test |
| M-06 | Native browser in a 360 px desktop window works in the sheet and hides under app menus and dialogs. Expand is hidden. | F4 | Real app with a synthetic page |
| M-07 | Without `window.tjommie`: the honest card, no Browser tab, Open link or menu item, and Take over as text. | F5 | Component test |
| M-08 | Exchange and members sheets; focus returns to the opener. | F7 | Component test |
| M-09 | Room posts fit and the mention popover stays inside the viewport. | F6 | Screenshot |
| M-10 | New bot and Bot settings in one column, picker inside the viewport, footer reachable, routine editor full screen. | F8 | Screenshots |
| M-11 | Settings list, then section with Back. Accounts cards, the full-screen account dialogs, and Usage tiles, bars and rows fit. Links only focus a button. | F9 to F13 | Component tests and screenshots |
| M-12 | Crossing the breakpoint with a sheet open or the browser expanded keeps state; `expanded` clears on entering phone width. | F3, F4 | Real app |
| M-13 | Light-theme spot-check of chat, Accounts and Usage at 390. | F2, F9, F12 | Screenshot |
| M-14 | 1440 × 900 is unchanged apart from Phase 2 additions. | none | Screenshot comparison |
| M-15 | No terminal or console window appears for any sign-in. Nothing from F14 or F15 is built. | F9, F11, F14, F15 | Review and real app |
| M-16 | Unknown-reset limit at 360: status, Retry queued work and its caption, Check again (Codex subscriptions only) all wrap without overflow. `action=retry` focuses the pill and never calls `accounts.retryLimit`. There is no automatic retry. | F9 | Component test and screenshot |
| M-17 | Usage shows provider cards resembling Paseo: available plan/account metadata, individual quota windows and reset times, and explicit refresh. Missing readings, credits or extra-usage metadata are not fabricated. The cards fit at 360 and 390. | F12 | Component test and real DOM |
| M-18 | Usage supports Today, This week, This month and This year (calendar year to date in the selected time zone). A separately labeled combined cost estimate sums the known costs across token usage and preserves partial/unknown totals. Actual API spend and subscription API value remain separately labeled. | F12 | Unit tests and real DOM |

## 11. UI-B implementation handoff

Start only when root authorizes it. Lane UI-B works in `packages/ui` only.

**Read first:** this file, the HTML (both sizes, both themes), `packages/ui/AGENTS.md` and the Next docs it points to, and the wireframe spec's "Colors and type".

**New files:**
- `components/tab-bar.tsx`
- `components/back-link.tsx`

**Edits:**
- `components/app-shell.tsx`: `h-dvh`, add `TabBar`, and stack vertically on phones.
- `components/rail.tsx`: `max-sm:hidden`.
- `app/page.tsx`: the phone redirect.
- `app/agents/page.tsx`, `components/agents/cards.tsx`
- `components/chat/chat-screen.tsx`, `chat-header.tsx`, `right-panel.tsx`, and `lib/panel-layout.ts` with its test (`isPhone`, phone bounds).
- `components/chat/browser-panel.tsx` (toolbar wrapping, the no-bridge state) and `bot-panel.tsx` (the Browser card's no-bridge variant).
- Wrap fixes in the composer, approval card, notices and bubbles.
- `components/exchange/exchange-view.tsx`, `components/room/*`, `components/dialog.tsx`
- `app/bots/new/page.tsx`, `app/bots/settings/page.tsx`, `components/bot-form/*`
- `app/settings/page.tsx` (the two-level phone layout), plus phone classes in `components/accounts/*` and `components/usage/*`.
- `lib/usage.ts` and its test (the §9 copy fix).
- `components/connection-screen.tsx`, `components/failed-load.tsx`

**Order:**
1. Shell, tab bar, back link and the `/` redirect.
2. Panel-layout phone mode and the sheets.
3. Dialog full-screen mode.
4. Screens F1 to F13.
5. The no-bridge browser state.

**Rules:**
- Never use `any`.
- No new dependencies.
- Token classes and the existing primitives only.
- Keep the existing native panes and their tests.
- No real accounts, keys, sign-ins or paid calls.
- Don't touch `packages/protocol`, `daemon` or `desktop`; `minWidth` is the coordinator's.
- Don't commit.

**Tests:**
- Tab bar: visibility by route, `aria-current`, the waiting-count name.
- The `/` redirect.
- `isPhone` and phone bounds.
- The chat panel defaults closed on phones and doesn't write `tjommie.panel`.
- Sheet focus in and out, and Escape.
- The no-bridge browser state (M-07).
- Dialog full-screen classes below 640.
- The usage copy fix.
- M-16: `action=retry` only focuses, and the retry pill and caption wrap at 360.

**Done when:** `npm run typecheck -w @tjommie/ui`, `npm run lint -w @tjommie/ui`, the UI vitest suite and the no-any check all pass. Report the changed files and test counts, and note anything that differs from this spec. Root runs the narrow real-app screenshot loop (M-01 to M-16) with synthetic data.

## 12. Open items for root

- **Unknown cost on an API key fails closed, on purpose.** The card explains the stop and offers no fix button. The current workaround is to add a new account. No new UI scope comes from this.
- **Caption clearance is hard-coded at 146 px.** Phase 3 clients will need it to drop to 16 px, probably through a CSS variable set only in the desktop window.
- **OpenPencil frames for this design are still to be drawn,** if the owner wants them in the `.fig` file as well.
