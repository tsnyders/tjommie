# Aegis: how to build with it

Aegis is a tokens-and-patterns design system: there are no components to import. Write your own markup and style it only with the Aegis CSS custom properties below. All of them come from `styles.css`, which also loads the fonts (Archivo, JetBrains Mono).

## Setup
- Load `styles.css`. Set the theme on the root element: `<html data-theme="light">`, or `data-theme="dark"` (the same palette in warm graphite).
- Phone layouts: add `data-device="phone"` to the root for phone type sizes.
- Reduced motion: `data-motion="reduced"` removes press, lift and enter offsets. Use it when the user prefers reduced motion.
- Bot colour: put `class="bot-blue"` (or `bot-purple`, `bot-pink`, `bot-gray`, `bot-teal`, `bot-green`, `bot-orange`, `bot-red`, `bot-brown`, `bot-amber`) on a bot's element, and use `var(--bot)` inside it.

## Styling idiom: CSS variables, no utility classes
- **Surfaces:** `--bg` (the stone frame), `--paper` (panels), `--raised` (white cards), `--sunk`, `--panel`.
- **Text:** `--ink`, `--ink-mid`, `--ink-muted`, `--text-2`, `--text-3`.
- **Lines:** `--line`, `--line-soft`, `--line-field`.
- **Primary action:** a near-black pill, `--primary` with `--on-primary`.
- **Accent:** one ochre meaning only, "something is waiting for you": `--accent`, `--accent-text`, `--accent-tint`, and `--marker` (the dot). Never use it for decoration or buttons.
- **Type:** font shorthands, e.g. `font: var(--type-body)`. Also `--type-display-l`, `--type-display-m`, `--type-display-s`, `--type-ui`, `--type-ui-strong`, `--type-label`, `--type-meta`, `--type-small`, `--type-code`, `--type-button`, `--type-nav`, `--type-figure`, `--type-wordmark`. Families: `--font-text`, `--font-display`, `--font-mono`.
- **Space:** `--space-0` to `--space-64` (2, 4, 6, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64).
- **Radius:** `--radius-xs`, `--radius-sm`, `--radius-md`, `--radius-field`, `--radius-slot`, `--radius-card`, `--radius-panel`, `--radius-hero`, `--radius-pill`.
- **Shadow:** `--shadow-card`, `--shadow-card-lift`, `--shadow-raised`, `--shadow-raised-focus`, `--shadow-lift`, `--shadow-glass`.
- **Size:** `--size-avatar-20`, `--size-avatar-28`, `--size-avatar-40`, `--size-avatar-64`, `--size-icon`, `--size-touch-min`.
- **Motion:** each spring is a ready-made duration plus timing function, e.g. `transition: transform var(--spring-calm)`.
  - `--spring-calm` for big surfaces, `--spring-snappy` for small parts, `--spring-playful` for the approve stamp.
  - Eases `--ease-out` and `--ease-move`, durations `--duration-base` and `--duration-fade`, stagger `--stagger-step`.

## Rules the design keeps
- The bots are the only colour. The chrome stays stone and ink.
- Text never scales or rotates. Nothing animates in response to a keyboard action.
- An amber bot always shows its name next to its avatar.
- On the phone a drawer replaces the bottom bar; on the desktop it is a persistent sidebar. A white composer card is pinned to the bottom of every chat.

## Where the truth lives
- `tokens/tokens.css` and `tokens/tokens.json`: every token, with values and notes.
- `guidelines/aegis-design-notes.md`: the full design notes (motion moments, contrast, navigation).
- The pattern cards in `components/<group>/<Name>/<Name>.html` and the full screens in `components/screens/`: read their source and copy the patterns.
- `guidelines/motion-clips/`: video references for the motion.

## Example
```jsx
<div className="bot-blue" style={{ background: 'var(--raised)', borderRadius: 'var(--radius-card)', boxShadow: 'var(--shadow-card)', padding: 'var(--space-16)' }}>
  <div style={{ font: 'var(--type-ui-strong)', color: 'var(--ink)' }}>Forge wants to run a command</div>
  <code style={{ font: 'var(--type-code)', color: 'var(--ink-mid)' }}>git push origin main</code>
  <span style={{ font: 'var(--type-meta)', color: 'var(--accent-text)' }}>Needs you</span>
  <button style={{ background: 'var(--primary)', color: 'var(--on-primary)', borderRadius: 'var(--radius-pill)', font: 'var(--type-button)', transition: 'transform var(--spring-snappy)' }}>Approve</button>
</div>
```
