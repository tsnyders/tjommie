# Building with Tjommie's components

Tjommie is a desktop and phone app for running AI bots: each bot has a blob avatar, a state (working, waiting for you, paused, limited, idle) and a chat. These components are the app's real ones, unchanged, from `packages/ui`.

## Setup and theme
- No provider is needed. Every component is styled by `styles.css`, which carries the tokens and the compiled component classes.
- Light is the default (`:root`). For dark, put `className="dark"` on the root element of the design and paint it with `background: var(--canvas)`. The tokens switch with it.
- Type is the system UI stack (`--font-sans`: Segoe UI Variable, Segoe UI, system-ui). The sizes in use are 11, 12, 13, 14, 15, 17 and 20 px, with weights 400, 500 and 600.

## Styling idiom
- Components bring their own classes. For your own layout glue, use inline styles with the CSS variables below. Don't invent Tailwind class names: only the classes these components use are compiled.
- Surfaces: `--canvas` (page), `--card` and `--card-stroke` (cards), `--popover` (menus), `--pill` (secondary pills), `--selected` (selected state), `--divider` (hairlines).
- Text: `--title` (headings), `--text` (primary), `--body` (reading text), `--secondary` and `--caption` (supporting).
- Meaning: `--accent` (waiting for you, links, focus), `--working` (working, done), `--danger` (errors). Waiting cards use `--waiting-fill` with a `--waiting-stroke` border.
- Bubbles: `--user-bubble` with `--user-text` for the person's messages; `--bubble` for the bots'.
- Radii: cards 12 to 14, menus and dialogs 16, rows 10, pills fully round.

## Components
- `BotAvatar` (`shape`, `color`, `size`): the bot's identity. It's the only decorative colour in the app; use the eight shapes and ten `BOT_COLORS`.
- `TeamMark` stands for the Team room. `MissingAvatar` stands in for a deleted bot.
- `Pill`: `primary` marks the one main action (light on dark, dark on light). `BAR_PILL` is the larger size for a page's action bar.
- `IconButton`: a 32 px icon action with a required `label`. `active` shows an open panel.
- `Segmented`, `Switch`, `Stepper`, `Label` with the `FIELD` input class: the form controls.
- `ActivityRow`: one line per turn of tool work ("5 steps: Tagged v2.4.0-rc.1").

Read `styles.css` and each component's `.d.ts` and `.prompt.md` before styling.

## Example

```jsx
<div className="dark" style={{ background: "var(--canvas)", padding: 16 }}>
  <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
    <BotAvatar shape="hexagon" color="orange" size={28} />
    <span style={{ color: "var(--title)", fontSize: 15, fontWeight: 600 }}>Night Shift</span>
    <span style={{ color: "var(--accent)", fontSize: 13 }}>Waiting for you</span>
  </div>
  <div style={{ display: "flex", gap: 8, marginTop: 12 }}>
    <Pill primary>Allow once</Pill>
    <Pill>Always allow</Pill>
    <Pill>Deny</Pill>
  </div>
</div>
```
