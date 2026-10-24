# Mobile navigation choices

**Selected:** Quiet pill (owner choice, 2026-10-22). Use this as the production mobile navigation direction. Retain these comparison files as the decision reference alongside the full screen handoff.

The owner confirmed that “Phase 2.1” means the mobile layout work previously named Phase 2.5, and requested at least three designs with pill-shaped bottom navigation before choosing a direction.

Open [the interactive preview](mobile-pill-options.html). It is a standalone local file with no external assets or network requests. Keyboard keys 1, 2 and 3 switch directions. The theme and size controls cover 390 × 844 and 360 × 640. Navigation, agent selection, search, filtering, draft review, sample composer and Settings subpages work locally. Choosing a direction displays its name; communicate that name in chat to select the implementation.

| Choice | Layout | Navigation | Tradeoff |
| --- | --- | --- | --- |
| Quiet pill | Compact rows with visible state labels | Wide floating pill, every destination labeled | Best density, restrained personality |
| Focus dock | Compact bordered cards | High-contrast floating dock; active destination expands with a label | Strongest navigation emphasis; inactive icons require recognition |
| Room to breathe | Generous cards and a lighter heading | Floating pill with blue active capsule | Calmer reading; more scrolling on short screens |

All three use the existing Agents, Team and Settings destinations. New agent is an action in the Agents header. Chat retains a way back to Agents. No API key, real account, approval execution or remote phone connection is present in the preview. Accounts and Usage have honest empty states. Sample messages and actions are identified as samples.

## Verification

- Rendered all 36 direction × size × theme × main-screen combinations in Chrome, without horizontal phone-frame overflow or page script errors.
- Exercised each direction’s navigation, filtering, search, agent opening, draft preview, sample composer and design selection.
- Inspected all three Agents screenshots visually.
- Nine screenshots under `mobile-pill-options/` show Agents, Chat and Settings for each direction. `preview.png` shows the comparison page.

This changes design files only. The installed Phase 2 desktop build is unchanged by this exploration. Mobile production integration follows the owner’s design selection; connectivity remains separate from the responsive layout.
