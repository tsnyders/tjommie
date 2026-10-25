# Tjommie mobile: five directions

- **Status:** a design proposal for the owner to choose from. No production code, native code, tests or packages changed. All bots, messages, accounts and figures in the prototype are made up.
- **Date:** 2026-10-22. **Branch:** `phase2-accounts-mobile-20261022`.
- **Asked for:** the owner's four complaints about the S25 build (grey system bars, the pill sitting in a band, the Team room and chat menu placement, and thin, distracting agent filters), then "redesign the whole mobile UI, five designs, for minimalists, people who love styles and so on, each with motion".
- **Desktop:** unchanged in every direction. All of this applies below 640 px or in the Android app only.

## Open it

`index.html` in this folder. It opens offline from disk and needs no build.

| Control | What it does |
|---|---|
| View | **One phone** (interactive, with notes); **All screens** (Agents, Team room, Chat, Settings and Usage side by side); **States** (menus, sheets, filter and keyboard); **Compare five** (one screen in every direction) |
| Direction | Hush, Folio, Aurora, Sticker, Console (keys 1 to 5) |
| Theme, Size | Dark or light (keys D and L); S25 at 411 × 891, or a narrow 360 × 800 phone |
| Screen, State | Agents, Team room, Chat, Settings, Usage (keys A, T, C, S, U), plus the states each one has |
| Bars | **Canvas** is the proposal. **Grey** is today's Android bars, for comparison |
| Motion | On, Reduced (what people with reduce-motion turned on get) or Off (still frames). Replay runs the entrance again |

Everything inside the phone works:
- Tabs, rows (opens the chat) and ⋯ (anchored menus).
- The chat or Team room title opens Details or Members as a sheet.
- Filters, Allow once (the card answers and the bot starts working), and Retry.
- The composer raises a keyboard and hides the pill. Enter sends and a sample reply follows. @ opens mentions.

## What every direction fixes

These come from the owner's screenshots, the UI Builder's and Design Lead's notes, and the current source. They're the same in all five directions; only the styling differs.

1. **System bars are the app's own canvas.**
   - The status bar and the gesture area show exactly what is behind them: the canvas, or Aurora's light.
   - Icons are light on dark themes and dark on light themes. They follow the theme chosen in Tjommie's Settings, not only the phone's.
   - Once something is under the header, a hairline appears below it. The **Bars** toggle shows today's grey for comparison.
   - The prototype also lets content scroll under both bars. That part is optional and separately scoped (see the Android section). The required part is that the bars read as canvas.
2. **The pill floats on its own.**
   - It is 60 px high and hugs its three tabs (about 232 px wide), centred, with `bottom: max(20px, bottom inset + 10px)`.
   - There is no band, tray or fade behind it. Lists pass beside and under it. Every overview scroller (Agents, the Settings list and sections, Accounts, Usage and the room list) leaves room to scroll its last row clear.
   - It shows only on Agents, Team room and Settings. It is never in a chat, New bot, Bot settings, sheets or dialogs. It hides whenever the keyboard is up.
   - The tab capsule moves to the tab you pick, and Agents keeps its waiting dot.
3. **Headers put actions in fixed places.**
   - Overview screens (Agents, Settings, Usage) use a large title with trailing actions on the bar above it.
   - Conversation screens use [back, chat only] [title button] [⋯]. The Team room is a top-level tab, so it has no Back.
   - The title block is left-aligned, truncates, and is itself the button that opens Details (chat) or Members (Team room).
   - ⋯ always sits flush right with an 8 px inset. Every header target is 44 px.
   - The desktop's 154 px caption-button gap is gone on phones. That gap is what put the Team room icon mid-header (`room-screen.tsx:100`).
   - Menus open under ⋯, anchored to it and right-aligned, with 46 px rows:
     - Chat: Details, Pause bot, Bot settings.
     - Team room: Members, Bots settings.
     - Agent rows: Open chat, Pause bot, Bot settings.
   - Open browser and Open home folder appear only where the desktop bridge exists.
   - The panel icon is dropped on phones: a full-screen sheet isn't a side panel.
4. **Filters are calm, content-sized and easy to hit.**
   - The full-width, 32 px segmented rail is gone.
   - Each direction has its own filter style (table below). All of them are at least 36 px tall, have a 44 px touch area, and show counts.
   - States with no bots get no filter. When only "All" would remain, the filter disappears.
   - At 360 px every row fits without scrolling. A row that ever overflows fades at its edge.
5. **Keyboard and safe areas.**
   - With the keyboard up, the composer sits 10 px above it and the pill fades away.
   - The list keeps its newest message in view only if you were already at the bottom.
   - In the Team room the composer floats 10 px above the pill, and the list's bottom padding covers both.

6. **Details keeps everything today's bot panel has, in every direction.**
   - The summary: account and model, Today's tokens and value, the limit with its reset, autonomy, and the home folder. Its Open link appears only with the desktop bridge.
   - Browser, Routines with Add, and Recent activity.
   - With the desktop bridge (a narrow desktop window), the sheet keeps the existing native Details/Browser pane. The "Runs on your PC" card is only for clients without it.
   - All sheets are `<aside>` and never dialogs, because app dialogs hide the native browser view.

Small product changes that ride along (each is visible in the prototype):
- Paused and limited rows say their state once. Today the chip and the task line repeat it.
- The Team room composer gets an @ button and the shorter placeholder "Message the Team room". Typing @ on a phone keyboard takes a layer switch, and the long placeholder was cut off at 360 px.
- The Settings list gains one-line subtitles: Appearance, the chain limit, account plans, and this week's **tokens**. The Usage row never shows a dollar figure, because spend and API-equivalent value stay separately labelled on the Usage screen.

The rest keeps today's copy and formats:
- Times use the app's en-US clock ("6:21 PM", "resets Wed 11:00 PM").
- Activity rows read "5 steps: Tagged v2.4.0-rc.1".
- Approvals, notices and empty states are unchanged.

## The five directions

| # | Direction | For | The idea | Agents filter | Motion signature | Build cost |
|---|---|---|---|---|---|---|
| 1 | **Hush** | Minimalists | Today's black and white with less chrome: hairline rows instead of cards, today's bubbles (bot #262626, radius 18), one accent for "waiting" | Outline chips with counts | Short fades with a 6 px rise; the capsule glides; working bots breathe a dot | Lowest |
| 2 | **Folio** | Minimalists who love type | A calm morning paper: serif headlines, rules, a one-line lede ("Night Shift is waiting for you…") | Text tabs and a sliding underline | Slower, softer easing; staggered entries; a red-pen circle draws around "Waiting for you" | Medium (one serif) |
| 3 | **Aurora** | People who love polished, glossy apps | Each bot casts its own light behind frosted glass; a "Needs you" card puts the waiting bot first | One glass "Show" button with a menu of counts | Springs; blur-to-focus entrances; the nav droplet stretches toward the next tab; halos turn around busy bots | Highest (blur needs a low-power fallback) |
| 4 | **Sticker** | People who want bold, playful style | The bots as characters: cards in each bot's colour, thick ink, hard shadows, highlighter yellow | Sticker chips with count badges | Bouncy springs; avatars blink, working bots bob; buttons press into their shadows | Medium (card grid) |
| 5 | **Console** | Developers and power users | Agents as processes: monospace, process-list rows with last activity, chat as a log, approvals show the exact command | Inverse-video tabs with counts | Step timing: wipes and line-by-line reveals, a braille spinner, a blinking LED, a faint scan on screen changes | Low to medium (one mono font) |

**Bubbles, on purpose:**
- Hush, Aurora and Sticker keep message bubbles.
- Folio sets bot replies as reading text and Console as a log. Both are deliberate departures from today's bubbles and are the owner's call.
- If either is chosen, `chat/bubbles.tsx` and `room/room-post.tsx` change.

Production type: Hush uses the system sans (Roboto on Android). Folio uses Newsreader, Sticker uses Archivo for display, and Console uses JetBrains Mono, each bundled as local font files. The prototype stands in with Windows fonts (Sitka, Franklin Gothic, Cascadia), so letterforms on the phone will differ slightly.

### Choosing

- **All five share one set of screens, components and behaviour.** Only tokens, a few layout rules and the motion layer differ, so they don't have to compete.
- **Ship one as the default** and, if the owner wants to court different tastes, **offer others under Settings → Appearance** later.
- **My pick for the default is Hush.**
  - It fixes every complaint while keeping the identity people already know.
  - It's the cheapest and the quickest to make solid.
- **For the attention-grabbing styles, add Sticker before Aurora.**
  - Sticker carries the most personality, it's built entirely from the bot colours and avatars, and it costs nothing at runtime.
  - Aurora is the most striking, but it needs a low-power fallback and checking on mid-range phones.

## Android system bars: required implementation

This is the same for every direction. The **visual requirement** is fixed: the two bar areas must look exactly like the canvas, with no band and no seam, and with icons whose contrast suits the chosen theme. The rest is the likely cause and fix, for root to prove on the S25 before freezing.

**Where the fix goes.** `packages/mobile/android/` is generated by `cap add android`. `cap sync` leaves `android/res` alone, but re-adding the platform loses hand edits. The authoritative places are:
- `packages/mobile/native/java/dev/tjommie/app/MainActivity.java`, which `scripts/apply-native.mjs` copies in;
- `packages/mobile/scripts/apply-native.mjs`, which already patches the manifest and Gradle. It should also write the window background into `values/` and create `values-night/`.

**Likely cause.**
- The generated `android/app/src/main/res/values/styles.xml` gives `AppTheme.NoActionBar` the parent `Theme.AppCompat.DayNight.NoActionBar` and `android:background @null`. It sets no window background and has no `values-night`.
- AppCompat's dark background is about `#303030`, which matches the grey bands in the screenshots.
- The app targets SDK 36, so it is edge to edge. The UI pads the shell by `max(env(safe-area-inset-*), var(--safe-area-inset-*))` (`.android-app` in `globals.css`), so whatever paints behind the bar areas shows through.

**Fix, in order** (stop when the screenshot proves it).

Steps 1 and 2 alone satisfy the complaint. With the shell still padded by the insets (today's `.android-app`), both bars read as canvas at rest.

Letting content scroll *under* the bars, as the prototype draws it, is optional and needs its own scope. It means dropping the shell's top padding and adding the top inset to every phone header: Agents, chat, Team room, Settings sections, forms, sheet bars and full-screen dialogs. Otherwise keep the safe shell padding.

1. Paint the window and the WebView with the canvas:
   - `android:windowBackground`: `#070707` in `values-night`, `#fcfcfc` in `values`.
   - Never set `android:background` at the theme level. Every view without its own background inherits it, including the system's text-selection toolbar. Its transparent popup then paints a solid band over the page. This happened on 2026-10-23, and the fix is in `apply-native.mjs`.
   - The WebView background set to the same colour. For the first paint, `android.backgroundColor` in `packages/mobile/capacitor.config.ts` can do this declaratively. Theme switches at runtime still need the native call in step 2.
   - `html` and `body` keep `background: var(--canvas)` all the way under the insets.
2. Sync bar icons with Tjommie's theme, not the phone's. Run it from `applyTheme` and `followTheme` in `lib/theme.ts`: on a Settings change, and on the System `matchMedia` listener when the phone flips. It calls native:
   - either Capacitor 8's SystemBars style call (its style names describe the bar background, not the icons: `DARK` means light icons for a dark app),
   - or a small method using `WindowInsetsControllerCompat.setAppearanceLightStatusBars` and `setAppearanceLightNavigationBars`.
   - In the same call, set the window background colour. A light Tjommie on a dark phone must not flash grey.
3. Keep contrast enforcement off for gesture navigation. Accept the system scrim on three-button navigation.
4. Keyboard: the manifest has no `adjustResize`. In an edge-to-edge app (target 36) the keyboard does not resize the WebView, so relying on `visualViewport` alone is only a hypothesis.
   - Expected result: the composer sits on the keyboard and the pill is gone.
   - Likely needed: a native WindowInsets listener in `MainActivity` reading `ime().bottom`. It either pads the WebView or pushes a `--ime-inset` CSS variable through `evaluateJavascript`. The same signal hides the pill.
   - Fallback without native code: hide the pill while a text field is focused.
   - No new keyboard dependency is assumed; `@capacitor/keyboard` would be one. Prove it on the device.
   - The Android Back event (`tjommie:back`) closes an open sheet or menu first.

**Acceptance on the S25, both themes:**
- Sampled pixels in the status bar and gesture areas equal the canvas.
- Bar icons are legible.
- Switching theme in Settings updates the icons immediately.
- No flash on launch.

## Motion rules (all directions)

- **Purpose only.**
  - Motion shows where you went (tab fade-through, chat push, back), where a menu came from (it grows out of ⋯), what changed (Allow once turns into an answer, then a working row), and what's live (working, waiting).
  - Nothing loops unless the state behind it is live.
- **Tabs fade through.** The old screen leaves in 90 ms and the new one starts 70 ms in, so two screens never overlap. Pushes lead in 40 ms.
- **Each direction keeps its own timing family:**
  - Hush: 160 to 300 ms ease-out.
  - Folio: 240 to 460 ms drawer curve.
  - Aurora and Sticker: physics springs, generated in `prototype.js`.
  - Console: steps.
- **No animation** on Send, on keyboard-initiated actions or on scrolling.
- **Reduced motion** keeps only short opacity fades. All movement, springs, halos, spinners and blinking stop, and every state is shown at rest.
- The values live in `prototype.css`. Each direction's block sets `--a-view`, `--a-item`, `--a-push`, `--a-menu`, `--a-sheet`, `--a-msg`, `--ind-ease` and `--ind-dur`. The motion modes are at the end of the file.

## Handoff to the UI Builder (after the owner picks)

Start only when root assigns it. Work in `packages/ui` (plus `packages/mobile` for the bars). Desktop must not change. Use phone-only scopes: `max-sm:`, `.android-app`, or "no desktop bridge". No `any`, no new dependencies (bundled font files only), no commits.

1. **Shared fixes first (any direction).**
   - `components/tab-bar.tsx` and `components/app-shell.tsx`: the pill becomes an overlay, not a row. Use `fixed`, centred, hugging its tabs, with `bottom: max(20px, inset + 10px)`. Every overview scroller gets bottom padding and `scroll-padding-bottom` of pill + gap + inset. Hide it (inert, aria-hidden) while the keyboard is up.
   - `components/room/room-screen.tsx`: the phone header follows the [title button][⋯] pattern. The title opens Members; ⋯ holds Members and Bots settings. Drop `PanelRight` and the `right-[154px]` offset on phones.
     - This is the only header that is wrong on Android today.
     - Agents and the forms already use `phoneCaptionClear()`, which is 16 px off-desktop.
     - The chat header's `right-[154px]` is in its desktop branch only; its phone branch already uses `captionClear()`.
   - `components/chat/chat-header.tsx`: the phone header keeps Back, title button and More, restyled.
     - Today More holds Pause or Resume, Open browser (already gated by the bridge), Open home folder and Bot settings.
     - Add Details first. It's new to the menu; the title already opens it.
     - Gate Open home folder on the desktop bridge. Today it calls `desktop()?.openFolder`, which does nothing on Android.
   - `components/menu.tsx`: 44 to 46 px rows below `sm`. Today `MENU_ITEM` is `h-9`.
   - `app/agents/page.tsx`: replace the phone `Segmented` with the chosen direction's filter, including the hide-when-useless rule. Keep `aria-pressed` and the group label "Show".
   - `components/agents/cards.tsx`: phone rows as drawn. State is shown once for paused and limited bots. ⋯ becomes a 44 px ghost button.
   - `components/room/room-composer.tsx`: add the @ button and the shorter placeholder.
   - `components/room/members-panel.tsx` and the chat Details panel (`right-panel.tsx`): sheets with a grip on phones.
     - They stay `<aside>`, never `role="dialog"`. On a narrow desktop window the native Browser pane lives in Details, and app dialogs hide it.
     - Never move or transform a sheet while the native Browser view shows. Keep Details full height when its Browser tab is selected, or show the grip only on Android and clients without the bridge.
     - Members already closes on Back and Escape through `useSheet`.
     - The chat Details panel has its own Escape but isn't on `useSheet`. Wire the Android Back event there; that's part of acceptance.
     - Details content stays at parity with `chat/bot-panel.tsx`: summary, Browser, Routines with Add, Recent activity.
   - `components/usage/usage-section.tsx`: the range becomes the direction's toggle row.
   - `app/settings/page.tsx`: the phone Settings list gains its one-line subtitles. The Usage row shows tokens, not dollars.
   - The system-bar fix above:
     - `packages/mobile/native/java/dev/tjommie/app/MainActivity.java`: window and WebView colour, bar-icon appearance, and the IME inset.
     - `packages/mobile/scripts/apply-native.mjs`: write the window background into `values/` and create `values-night/`. Never edit the generated `packages/mobile/android/`.
     - A theme bridge from `applyTheme` and `followTheme` in `lib/theme.ts`.
2. **Then the chosen direction.**
   - Duplicating a direction's tokens under a phone selector is fine for Hush.
   - Aurora's blur needs a low-power fallback gated on a class. `prefers-reduced-transparency` isn't widely supported.
   - Bundled local font files work in the static export, but they add to the APK size.
   - Copy its token block from `prototype.css` into `globals.css`, scoped to phones.
   - Map components to token classes and add its layout rules (for example Sticker's two-column card grid).
   - Add its motion block, gated by the existing phone motion and reduced-motion rules.
3. **Prove it.**
   - UI tests for the pill (routes, `aria-current`, the waiting name, hidden with the keyboard), the header menus and the filter rules.
   - `npm run typecheck`, `lint` and the UI suite.
   - Root runs the S25 checks: bars, pill clearance, keyboard, both themes, 411 and 360.

## Verification done

- Rendered in headless Chrome:
  - 5 directions × 5 screens × 2 themes at 411.
  - All five at 360 in both themes.
  - 8 states per direction.
  - All five side by side per screen in both themes.
  - The bars before and after.
- No page errors. No horizontal page overflow anywhere. Every filter and range row fits at 411 and 360. Nothing tappable is under 36 px, and the hit areas are 44 px.
- Motion was checked from exact frames. The animations were paused and seeked to 0, 60, 120, 200, 320 and 600 ms for: a tab switch, opening a chat, opening ⋯, and raising the keyboard. That's how the Folio tab change was caught overlapping, and it's now a fade-through.
- Reduced motion was checked the same way on Aurora, the most animated direction. Tabs, pushes and menus become opacity fades only, with no blur, scale or spring. Halos and the background light stand still, and the keyboard appears without sliding.
- **Known limits:**
  - Fonts are Windows stand-ins.
  - The keyboard is simulated.
  - Glass blur cost and the bar fix still need the real S25.
  - Contrast was checked by eye at render size, not with a meter.

## /design-sync

The `/design-sync` skill (bundled, version 2.1.304) was available and was invoked. It does not draw screens. It imports a component library into a claude.ai/design project, so the Claude Design agent can build with real components.

**Uploaded on 2026-10-22, after the owner ran `/design-login`.** The project is "Tjommie UI": https://claude.ai/design/p/163cd1ab-5122-42d5-8216-715d803afc5c.
- It holds 10 of today's real components (avatars, pills, buttons, form controls, the activity row) with the app's own tokens: 58 files.
- Every preview was graded and the render check is clean. The owner accepted the system monospace font for code.
- Once the chosen direction is built, re-sync so Claude Design builds with the new look.
- Details and re-sync steps are in `sync-staging/STATUS.md`.

## Files

- `index.html`, `prototype.css`, `prototype.js`: the interactive prototype.
- `shots/<direction>-screens-<dark|light>.png`: five screens at 411.
- `shots/<direction>-screens-360-<dark|light>.png`: five screens at 360.
- `shots/<direction>-states-dark.png`: filter, row menu, Team ⋯, Members, keyboard (Team), chat ⋯, Details, keyboard (chat).
- `shots/compare-<screen>-<dark|light>.png`: one screen in all five directions.
- `shots/motion-<direction>.png`: frame strips for four transitions. `shots/motion-aurora-reduced.png` is the reduced-motion version.
- `shots/bars-before.png`, `shots/bars-after.png`: today's grey bars and the canvas.
- `sync-staging/`: the isolated `/design-sync` pipeline.
  - `STATUS.md` holds the state, the blocker and the exact upload plan.
  - The package itself: sources copied verbatim from `packages/ui`, plus `build.mjs`.
  - `.design-sync/` holds the config, the authored previews, `conventions.md` and `NOTES.md`.
  - Generated output (`dist/`, `.ds-sync/`, `ds-bundle/`, caches) is git-ignored.
