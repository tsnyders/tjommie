# Mobile Appearance delivery

Owner selected Hush, Folio and Aurora after requesting installation on Windows and the S25 Ultra. Ship three mobile Appearance choices with Hush default. Preserve desktop layout and functionality, pairing, account data, routine controls and usage reporting.

## Ownership

- UI Builder: `packages/ui`, excluding root's narrow `lib/mobile.ts` native appearance wrapper. Phone-only styles, navigation, filters, menus, effects preference, style persistence and tests.
- Root: `packages/mobile` native canvas/icon bridge, IME observation, generated-project application, UI native wrapper, real-app tests, packaging and installation.
- Designer: read-only fidelity guidance/review. No sync upload until installed design is frozen.

## Native contract

`TjommieAppearance.setCanvas({color: '#RRGGBB', dark: boolean})` persists the opaque canvas and icon contrast, paints the window and WebView, and follows the app choice across OS configuration changes. `getInsets()` also publishes `window.tjommieInsets` and `tjommie:insets` with `{imeBottom, keyboardVisible}` in CSS pixels. Capacitor 8.5.3 SystemBars already owns decor IME resizing; this bridge observes it and adds no second padding. The UI uses visibility to hide the floating pill.

## Acceptance

- Each style: Agents, Team room, chat, Details, Settings, Usage, account/dialog controls; dark and light at 360 and 411 px. Hush bubbles retained. Folio local licensed serif. Aurora reduced-effects solid fallback.
- Floating nav has no backing tray; final list rows and Team composer clear it. Keyboard keeps composer reachable and hides nav. Long names and drafts remain bounded.
- System bars match the active style canvas and icon contrast after style/theme change and reload. Safe areas remain correct.
- Renderer errors, reduced motion, unchanged desktop styles, existing scroll/draft/reconnect regressions checked with synthetic unsigned accounts.
- Package input hashes and payload verification, Windows packaged smoke, physical Android touch/IME/style/reconnect proof, then install both tested artifacts. Preserve owner pairing; no real provider turns or authentication changes.

## Evidence

- UI Builder's full suite passed 58 files / 424 tests, followed by scoped checks for review fixes; UI typecheck, eslint and explicit-any checks passed. Root TypeScript passed; 379 TypeScript files passed the explicit-any check.
- Three styles in both themes passed actual renderer checks at 360 and 411 px, including persistent selection, floating nav bounds, hidden navigation under Members, Details routines and unchanged 1100 px desktop canvas. Real screenshots are under `docs/screenshots/phone/appearance/`.
- The source phone-layout suite passed forms, account dialogs, exchanges, focus return and native Browser integration. Existing real draft-preservation and mobile-motion suites passed.
- Physical screenshots cover all three styles in light/dark, including canvas-coloured system bars and the floating pill. Hush/Folio bar samples matched their exact canvas.

## Repeated-reload connection fix

The first two physical runs reproduced a native socket leak: full document navigation/reload reset Capacitor JS listeners but left sockets in `TjommieSocketPlugin.connections`, reaching the eight-wire limit. The style test's multiple reloads exposed it before final delivery.

`TjommieSocketPlugin` now retires active and pending sockets on `WebViewListener.onPageStarted`. A lifecycle lock atomically reserves active plus pending capacity, tracks pending sockets before connect, and serializes promotion with a document generation. Old opens, frame/closed events and asynchronous send replies cannot publish into a newer page. The executor survives page reload and shuts down only on plugin destruction; the listener is removed then. The independent narrow source review is clear. Native compile and JVM checks passed; physical repetition is in progress.

The capture helper also avoids sending Android Escape when no keyboard is showing, which could otherwise act as Back during a screenshot step. This is test-harness behavior, separate from the native connection leak.

### Final registration correction and device result

The first lifecycle patch still reached the native socket cap. Physical diagnostics recorded ten accepted host connections and native `open: limit` errors. Capacitor's `Bridge.Builder` replaces its WebView listener list after constructing the Bridge and loading plugins, dropping listeners registered during `load()`. Socket and appearance listeners now register in `handleOnStart`, after construction, using remove/add to avoid duplicate registrations on resume. The independent reviewer traced the actual Builder/Activity ordering and cleared this correction.

The physical S25 suite then passed (one test, 78.06 seconds): all six style/theme reloads and persistence, real swipe scrolling in the bot form and long conversation, bot creation, Team room mentions and Android Back, chat approval/send, Details Back, Usage/year, reconnect, and revocation. It used disposable paused agents without provider turns. Its synthetic host was forgotten; one owner host remained and the app returned to that connection.

IME limitation: this handset suppressed the keyboard for injected taps, while the owner confirmed it opens normally by hand. The passing run explicitly used `TJOMMIE_ANDROID_MANUAL_IME=1`; automatic IME visibility and nav-hiding assertions were not claimed as passed. The strict default gate remains enabled for normal runs. Native compile and 12 JVM tests passed; TypeScript and the no-any check (379 files) passed.

## Installed delivery

Final packages were rebuilt with frozen inputs. Windows packaged smoke passed (one test, 7.31 seconds). The installed Windows payload matches all 573 packaged files, and its daemon is listening on 47100. Installation started only after a read-only check found zero running work items.

The installed Android APK SHA256 matches the final package (`f88e1119e9ff2df97f2a17a345a757c9b183d572cb26c679deb3d112a79a1156`); packaging verified all 75 bundled UI assets. After the Windows installer finished, the final APK connected to the owner's actual PC and passed three further document reloads. Hush remains selected, one owner host is saved, and no synthetic test hosts remain. Android is a debug-signed local-test build.

One earlier disposable test-folder deletion was rejected by automatic approval review without a detailed reason. Its process and synthetic credential were cleaned up; the folder was left in place.

## Text selection and review polish follow-up

Claude restored the Android theme's `android:background` to `@null`, retaining only window/runtime canvas painting. This avoids an opaque canvas inherited by the floating selection popup. Applying the generated-theme patch twice is idempotent. Root also found that Android Back navigated away while text was selected; MainActivity now clears a noncollapsed selection before overlay/history handling. The independent narrow source review found no regressions.

The physical S25 suite passed again (92.51 seconds), including native long-press selection in Team room and chat, followed by Back clearing selection without changing route. Screenshots `android-selection-room.png` and `android-selection-chat.png` show the normal rounded native toolbar and no blank band. All six style/theme captures and the existing swipe/chat/approval/reconnect checks passed; the prior manually confirmed IME exception remains explicit.

Claude's polish includes 44px phone Usage chips and row menus, Folio typography/bubble/file-card details, and calmer, more legible Aurora colours. The extended real renderer suite passed in 38.75 seconds: all three styles in both themes at 360/411px, Usage chips fitting without horizontal scrolling, 44px settled control bounds, and unchanged desktop canvas. TypeScript, the no-any check (379 files), and 12 JVM tests passed. Final Windows packaged smoke passed (7.22 seconds); Android packaging verified 75 UI assets.
