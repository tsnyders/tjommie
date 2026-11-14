# Design sync notes: Aegis → Claude Design "Tjommie Aegis"

Project: https://claude.ai/design/p/43494ba1-c221-46f0-8d6e-56d531563d26. Source kit: `design/claude-design/aegis/`.

## The recipe (off-script layout)
Aegis has no React components. It is made of tokens, plus hand-authored HTML pattern cards and screens. The converter runs in tokens-only mode and builds the base files, and two small scripts wrap it. Run these from the repo root:

1. `node .design-sync/aegis-prepare.mjs`: builds the throwaway package `.ds-sync/aegis-nm/tjommie-aegis/` (the Google Fonts `@import` plus `tokens.css`, saved as `aegis.css`).
2. `node .ds-sync/package-build.mjs --config .design-sync/config.json --node-modules .ds-sync/aegis-nm --out ./ds-bundle`: prints `[ZERO_MATCH] … treating as tokens-only DS`, which is expected.
3. `node .design-sync/aegis-cards.mjs`: adds the 16 component cards and the 6 screens (as the "Screens" group), `tokens/`, `guidelines/` (the README as `aegis-design-notes.md`, plus the motion clips and shots). It deletes `_ds_sync.json` and `.stories-map.json`, and stamps `componentCount` into `.ds-build-meta.json`.
4. `node .ds-sync/package-validate.mjs ./ds-bundle`: must exit 0 with all 22 previews rendering.

## Setup gotchas
- `.ds-sync/aegis-nm/` needs `react`, `react-dom` and `scheduler` **copied** (not linked) from `.ds-sync/node_modules`. Without them the build fails with "react not found under --node-modules".
- The staged playwright must match the cached Chromium (playwright@1.63.0 ↔ chromium-1243).
- In the PowerShell tool, `Set-Location` into `.ds-sync` moves the shared working directory. Run everything from the repo root.

## Known render warnings (triaged, legitimate)
- `[RENDER_THIN] components/foundations/Motion/Motion.html: variants render identically`: the Motion card differs only in animation, so its screenshots match.
- `[FONT_REMOTE]` Archivo / JetBrains Mono: they are served by Google Fonts at runtime, by design.
- `_ds_sync.json absent`: this is intentional for this off-script layout (see below).
- PhoneChat and PhoneAgents are live scripted animations. The render check captures their first frame, and they fill in after about 4 s. This is not a defect.

## Why there is no `_ds_sync.json`
The converter's anchor hashes only its own build, which has no cards. Uploading it would vouch for files it never hashed. With no anchor, every re-sync re-verifies all 22 cards, which is cheap at this size.

## Re-sync risks
- The cards and screens are copied verbatim from `design/claude-design/aegis/`. If that folder gains a component without a first-line `<!-- @dsCard group="…" -->`, `aegis-cards.mjs` throws.
- `conventions.md` names tokens by hand. After any change to `tokens.css`, re-check that every `--name` in it still exists in `ds-bundle/_ds_bundle.css`.
- The fonts come from the network (Google Fonts). Offline renders fall back to system fonts.
- The project has no anchor, so deletes can't be derived. On a re-sync, review the project's `list_files` for paths this build no longer produces, and put them in the plan's `deletes`.
- If the screens' links between each other change shape (anything other than `href="name.html"`), the rewrite in `aegis-cards.mjs` misses them.
