# Mission Control

A Tjommie design direction for the Rust rewrite (Phase 0). Slug: `mission-control`. Primary theme: dark.

## The idea

Tjommie as an instrument panel for someone supervising a fleet of bots. The chrome is graphite and stays quiet, so colour can carry exactly two things: who a bot is (its ten identity colours, on the app's own avatar shapes) and what state it is in (six status hues, each with its own glyph). "Needs you" is the only solid fill in the whole system, so a request for the operator can't be mistaken for anything else, and the same yellow lamp shows up in the title bar triage key, the fleet sidebar, the roster, the chat, the composer hint and the phone's tab badge. Every number, time, path, command and quota is set in a condensed monospace and read off segmented gauges, the chat is a log with a time gutter, and keyboard paths are printed on the controls they drive (Ctrl+K, Ctrl+J, Alt+A, Alt+D, Ctrl+1 to Ctrl+9).

## Files

| Path | What it is |
|---|---|
| `tokens.json` | Source of truth: colour (dark and light), the 10 bot colours per theme, type scale (desktop and phone), spacing, radius, sizes, elevation, motion. |
| `tokens.css` | Generated from `tokens.json` by `build.mjs`. Do not edit by hand. |
| `kit.css` | Shared component styles. Uses only `tokens.css` variables (the drawn web page inside the browser mock uses `page-*` tokens and website-like sizes, because it stands for remote content). |
| `build.mjs` | `node build.mjs` writes `tokens.css`, checks every text and glyph colour pair for contrast (exits 1 on a failure), writes `screens/desktop-chat-alt.html` from `desktop-chat.html`, and inlines the exact Lucide icons and the app's bot avatar paths into each HTML file so every file is self-contained. `node build.mjs --shots [name]` renders the PNGs with headless Edge. |
| `components/*.html` | One sheet per group, first line `<!-- @dsCard group="..." -->`: `color`, `type` (Foundations); `app-shell`, `host-switcher` (Navigation); `bot-avatar`, `status`, `quota-bar` (Bots); `message`, `approval-exchange`, `composer` (Chat); `browser` (Browser); `file` (Files); `overlays` (Overlays); `forms` (Forms); `task-card` (Projects). |
| `screens/*.html` | `desktop-chat` (1440x900), `desktop-agents` (1440x900), `phone-chat` and `phone-agents` (412x892), all dark; `desktop-chat-alt` is desktop chat on cool paper (light). |
| `shots/*.png` | Screens: desktop at 1x, phone at 2x. `shots/components/*.png`: every component sheet. |

## Status semantics

| BotState | Label | Hue | Glyph (Lucide) | Treatment |
|---|---|---|---|---|
| waiting | Needs you | signal yellow | circle-alert | Solid fill with ink text; in light theme a dark amber edge keeps 3:1 on paper |
| working | Working | green | loader-circle, spins only while working | Tint and line |
| limited | Limited | violet | hourglass | Tint and line. Never red: a limit is a pause, not an error |
| paused | Paused | slate | pause | Tint and line |
| needs_setup | Needs setup | blue | plug | Tint and line |
| idle | Idle | dim neutral | circle-dashed | Outline only, no avatar badge |

Red is kept for failures, destructive actions and the live and recording lamps. Task and check results reuse the same hues (green passed or progressing, red failed or blocked, neutral not started). Autonomy (Ask, Folder, Full) is a three-segment gauge with its word beside it.

## Keyboard map (shown on the controls)

| Keys | Action |
|---|---|
| Ctrl+K | Command palette: bots, rooms, commands |
| Ctrl+J | Go to the oldest request that needs you |
| Ctrl+1 to Ctrl+9, Ctrl+0 | Jump to a bot by sidebar position; Agents overview |
| Alt+A, Alt+D | Approve or deny the newest request in the open chat |
| Enter, Ctrl+Enter, Ctrl+. | Send (Steer while a bot works), Queue, Stop |
| Ctrl+M, Ctrl+U, Ctrl+B | Dictate, attach files, browser panel |
| Ctrl+Shift+T | Take control of the bot's browser, and return it |
| Ctrl+N, Ctrl+T, Ctrl+, | New bot, new task, settings |

While you are in control of a page, the app must see its own shortcuts before forwarding keys to the page.

## Rendering table

Every visual effect used in the HTML, and how each native toolkit draws it. Nothing below needs blur, filters, blend modes, radial or conic gradients, text clipping or noise.

| Effect | Where | GPUI | Jetpack Compose |
|---|---|---|---|
| Solid fills | Surfaces, pills, lamps, buttons, badges | `div().bg(rgb(..))` | `Modifier.background(color, shape)` |
| 1 px hairlines and 2 px frames | Rows, cards, inputs, the bot-colour browser frame | `.border_1()` / `.border_2()` with `.border_color(..)` | `Modifier.border(1.dp, color, shape)` |
| Per-corner radii, 0 to 12 | Everything; phone sheet top corners only | `.rounded_*()` or `corner_radii(Corners { .. })` | `RoundedCornerShape(topStart = .., ..)` |
| Box shadows | Menu, toast, dialog, phone sheet only | `.shadow(vec![BoxShadow { color, offset, blur_radius, spread_radius }])` | `Modifier.dropShadow(shape, Shadow(radius, spread, color, offset))` (Compose UI 1.9+), fallback `Modifier.shadow(elevation, shape, ambientColor, spotColor)` |
| Spread rings (no blur) | Focus ring: 2 px gap in the surface colour, then 2 px focus ink; avatar badge cut-out | `BoxShadow` with blur 0 and spread, or an outer div with a border | Nested `Modifier.border(..)` with `padding`, or `dropShadow` with radius 0 |
| Colour with alpha | Dialog and sheet scrim | `hsla(..)` fill | `Color(..., alpha = ..)` |
| Linear gradient | 40 px right-edge fade on code that scrolls sideways | `linear_gradient(..)` background | `Brush.horizontalGradient(..)` |
| Bot avatar SVG paths | Avatars, 16 to 64 | Two stacked single-colour `svg()` layers (body tinted with the bot colour, eyes tinted white), because GPUI tints an SVG with one colour | `ImageVector` with two paths, or `Canvas` with `PathParser().parsePathString(d).toPath()` |
| Lucide icons, 2 px stroke | All icons | `svg().path(..).text_color(..)` | `Icon(imageVector, tint = ..)` |
| Bundled fonts, two widths plus mono | All text | Register the TTFs, then `font_family("Archivo SemiCondensed")`, `"Archivo"`, `"Martian Mono Condensed"` | `FontFamily(Font(R.font.archivo_semicondensed_regular, FontWeight.Normal), ..)` |
| Tabular figures | Every number | `FontFeatures` with `tnum` | `TextStyle(fontFeatureSettings = "tnum")` |
| Single-line truncation | Names, paths, activity lines, tab titles | `.truncate()` | `maxLines = 1, overflow = TextOverflow.Ellipsis` |
| Segmented gauges | Quotas, spend, dictation level, autonomy, project progress | A row of small divs | A `Row` of `Box`es, or one `Canvas` |
| Rotation loop | Working glyph, only while a bot works | `with_animation` and `Transformation::rotate` on the `svg()` | `rememberInfiniteTransition` and `Modifier.rotate(..)` |
| Opacity loop | Live and recording lamps, only while live | `with_animation` driving `.opacity(..)` | `rememberInfiniteTransition` driving `Modifier.alpha(..)` |
| Eased transitions | Hover, menus, panels, dialogs, sheets | `Animation::new(d).with_easing(..)` with a cubic-bezier(0.2, 0, 0, 1) function | `tween(d, easing = CubicBezierEasing(0.2f, 0f, 0f, 1f))` |

Rules the HTML follows so the table stays true:
- No letter-spacing anywhere (GPUI's text style has no tracking), so there are no tracked uppercase labels.
- No inset shadows and no dashed borders. Hollow lamps are a 1 px border.
- GPUI clips to rectangles, so a child with a fill inside a rounded card carries its own matching corner radii instead of relying on a rounded clip (Compose may use `Modifier.clip(shape)`).
- The live browser view is a native child view that can't be clipped round. Its frame is 2 px on a 2 px radius, so the opening is square.
- The title bar draws the Windows caption buttons itself (46 px each, Close turns red on hover). The maximise button should answer `HTMAXBUTTON` so Windows 11 Snap Layouts still appear.

## Fonts and licences

| Family | Role | Files to bundle | Licence |
|---|---|---|---|
| Archivo, Omnibus-Type ([source](https://github.com/Omnibus-Type/Archivo)) | Interface chrome at Semi Condensed width; message text at normal width | `ArchivoSemiCondensed-Regular/Medium/SemiBold.ttf`, `Archivo-Regular/Medium/SemiBold.ttf` (official static instances) | SIL Open Font License 1.1 |
| Martian Mono, Evil Martians ([source](https://github.com/evilmartians/mono)) | Numbers, times, paths, commands, quotas, code, key caps | `MartianMonoCondensed-Regular.ttf`, `MartianMonoCondensed-Medium.ttf` (static instances in the v1.1.0 release) | SIL Open Font License 1.1 |
| Lucide | Icons (not a font) | Exported SVGs, one family | ISC |

The HTML loads both families from Google Fonts as variable fonts and selects the widths with `font-stretch` (semi-condensed 87.5%, condensed 75%), which match the static files above. The browser mock's page text uses Georgia and Consolas only because it pictures a remote web page; the app never draws page content with its own fonts.

## Motion

- Hover and press colour: 90 ms. State pill swaps and menus opened with the pointer: 150 ms. Panels, dialogs and the phone sheet entering: 220 ms (fade plus an 8 px slide; the phone sheet rises from the bottom). All use ease-out, cubic-bezier(0.2, 0, 0, 1). Nothing exceeds 300 ms.
- Loops run only while their state is true: the working glyph spins (900 ms, linear) while a bot works; the live and recording lamps pulse in opacity (1600 ms) only while a stream or microphone is live. The dictation level meter follows the real input level; it is data, not decoration.
- Nothing animates on keyboard actions: the palette, Ctrl+J, Ctrl+1 to Ctrl+9, Alt+A, Alt+D and arrow navigation change state in one frame.
- Reduced motion (Windows "Animation effects" off, Android "Remove animations"): every transition is instant, the working glyph is a static arc, live lamps stay steady, and the level meter updates at 4 Hz without easing.

## Contrast

`node build.mjs` checks, in both themes: text, text-2 and text-3 on every surface at 4.5:1; each status colour on every surface and on its own tint at 4.5:1; the Needs you ink on yellow at 4.5:1; badge glyphs, lamps, the focus ring, the live lamp and all ten bot colours at 3:1. All pairs pass. Light-theme bot colours are slightly darker, most of all orange, amber, green and teal, so the avatars hold 3:1 on paper; every hue stays recognisable.
