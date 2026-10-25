# design-sync notes: Tjommie UI kit (staging)

- **Source.** `src/components/*` are verbatim copies of `packages/ui/components/{bot-avatar,pill,icon-button,controls}.tsx` and `chat/activity-row.tsx`, and `src/lib/utils.ts`. `src/globals.css` is a verbatim copy of `packages/ui/app/globals.css`. Re-copy them before every re-sync, then `cmp` each against its original.
- **Build.** `node build.mjs` writes three things to `dist/`:
  - `index.js` with esbuild, from `.ds-sync/node_modules`; the dependencies stay external.
  - the `.d.ts` files, with the repo's own `typescript`. `tsconfig` sets `allowImportingTsExtensions`, because `@tjommie/protocol`'s sources import with `.ts` extensions.
  - `styles.css`, from Tailwind 4.3.3 (`@tailwindcss/node` and `@tailwindcss/oxide`) compiling `globals.css` against `src/`.
  - Nothing is installed outside `.ds-sync/`. Everything else resolves read-only from the repo's `node_modules`.
- **Converter.** Run it from this folder with `--node-modules <repo>/node_modules --entry ./dist/index.js --out ./ds-bundle`.
- **Browser.** The repo's Playwright (1.64 alpha) pins a Chromium build that isn't cached on this machine. Set `DS_CHROMIUM_PATH` to the installed Chrome (`C:/Program Files/Google/Chrome/Application/chrome.exe`) for validate and capture. No browser download is needed.
- **Previews.** Previews use inline styles with the token variables for layout glue. Only classes the components themselves use are compiled into `styles.css`, so a Tailwind class used only in a preview would not exist.
- `Segmented` uses `cardMode: column`. Its Range story is wider than a grid cell (`[GRID_OVERFLOW]`).

## Known render warns
- `[FONT_MISSING] "Cascadia Mono"`. It's the app's markdown code font, a Windows system font with no `@font-face`. None of the 10 synced components uses it.
  - **Owner decision, 2026-10-22: accept the system monospace substitute.** Expect this warn on every sync. To change the decision, ship the OFL font via `cfg.extraFonts`.
- Body type is the system UI stack (Segoe UI Variable, Segoe UI, system-ui). That's by design, not a missing brand font.

## History
- 2026-10-22: first sync to "Tjommie UI" (`163cd1ab-5122-42d5-8216-715d803afc5c`) on the incremental path. 58 files, and the list matched afterwards.
- `/design-login` has to be run once by the owner in an interactive Claude Code session. Headless runs then reuse it.

## Re-sync risks
- The sources are copies, so they go stale as soon as `packages/ui` changes. The owner is choosing a new mobile direction, so this kit shows **today's** Quiet components.
- `@tjommie/protocol` types resolve from the repo's workspace link. A protocol change can change the emitted `.d.ts` files.
- Only 10 presentational components are in scope. App-coupled components (chat, room, agents cards, tab bar) need Next and store shims and are excluded.
- The grades are local (`.design-sync/.cache/review/`). Nothing has been uploaded, so there is no remote anchor yet.
