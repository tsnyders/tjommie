# /design-sync status

- **Skill:** `design-sync`, bundled with Claude Code 2.1.304. It was available and invoked.
- **What it does:** it imports a React component library into a claude.ai/design project, so the Claude Design agent builds with the real components. It does not draw screens.
- **Required notice:** "Standalone Claude Design (claude.ai/design) closes by December 14, 2026. Claude Design now lives in Claude."

## State: uploaded, 2026-10-22

- **Project:** "Tjommie UI", https://claude.ai/design/p/163cd1ab-5122-42d5-8216-715d803afc5c
  - A design-system project owned by the owner.
  - Its id is pinned in `.design-sync/config.json`.
- **Uploaded:** 58 files and 10 components. The project's file list matches the local bundle exactly. `report_validate` was filed with total 10, bad 0, thin 0, identical variants 0, and 5 rebuild passes.
- **How it got here:**
  - The first attempt was blocked: `/design-login` can't run in a non-interactive session.
  - The owner ran `/design-login`, then chose in this session: upload now, the name "Tjommie UI", and the system monospace substitute for the code font.
  - `create_project` and `finalize_plan` went through their approval prompts. The plan id is `plan_163cd1ab512242d5_be2541717de8` (incremental path).
  - The upload went in the skill's order: sentinel, content (6 base and vendor files, then 50 component and preview files), reconciliation (nothing to delete), sentinel re-armed, `_ds_sync.json` last.

## What was synced

10 real presentational components, copied verbatim from `packages/ui`:
- BotAvatar, MissingAvatar, TeamMark;
- Pill (with BAR_PILL), IconButton;
- Label (with FIELD), Switch, Segmented, Stepper;
- ActivityRow.

The tokens and styles are the app's own `globals.css`, compiled with Tailwind 4.3.3. This is **today's** Quiet design.

## Verification before upload, all inside this folder

| Step | Result |
|---|---|
| Install | `esbuild`, `ts-morph` and `@types/react` went into `.ds-sync/` only. The root `package.json` and `package-lock.json` hashes are unchanged. |
| Build | `node build.mjs` produces `dist/index.js`, the `.d.ts` tree and `dist/styles.css`. |
| Convert and validate | Both exit 0. Render check: 10/10 clean. 31 authored story cells, all graded `good` from their review sheets. |
| Conventions | `.design-sync/conventions.md` is wired through `readmeHeader`. Every token and component it names is checked against the built CSS and bundle. |
| Final driver | Run after the project id was pinned: `ok`. All 10 grades carried forward, 0 cleared, 0 pending, no unmerged learnings. |
| Accepted warning | `[FONT_MISSING] "Cascadia Mono"`. The owner accepted the system monospace substitute (see NOTES.md). |

Human review page: `ds-bundle/.review.html`. Serve it with `node .ds-sync/storybook/http-serve.mjs ./ds-bundle` from this folder.

## Re-sync (after the chosen mobile direction is built)

1. Re-copy the sources from `packages/ui` (see NOTES.md), then run `node build.mjs`.
2. Fetch the project's `_ds_sync.json` into `.design-sync/.cache/remote-sync.json`.
3. Run the driver with `--remote` pointing at that file:
   ```sh
   DS_CHROMIUM_PATH="C:/Program Files/Google/Chrome/Application/chrome.exe" node .ds-sync/resync.mjs --config .design-sync/config.json --node-modules ../../../../node_modules --entry ./dist/index.js --out ./ds-bundle --remote .design-sync/.cache/remote-sync.json
   ```
4. Grade anything new, then upload. A pinned project always uses the atomic path, and deletes come verbatim from `upload.deletePaths`.

## Not done

- The durable sync inputs are **not committed**, per root's no-commit rule. Those are `config.json`, `NOTES.md`, `conventions.md` and `previews/` in `.design-sync/`, and the package files in this folder. Commit them when root or the owner decides.
