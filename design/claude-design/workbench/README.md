# Workbench

**Direction A for the Tjommie Rust rewrite, inspired by quiet.md, especially its motion.**

Your bots' work lies on a quiet bench as objects you can pick up. Bot cards show a live miniature of each bot's latest chat lines or its browser. Rooms and projects are stacks of paper. You carry a card to a stack, the sheets part to take it, and the stack settles. The chrome stays monochrome on a cool near-white canvas, or a soft graphite one in the dark theme. One clean sans does all the type, and colour comes only from content: the ten bot colours, code and the browser page. Everything moves like paper on a spring: it lifts, fans and settles, it starts on the frame you touch it, and you can grab it again mid-flight.

## Feel it

Open the HTML files in Edge or Chrome to feel the motion; the PNGs in `shots/` only show the resting state.

- `screens/desktop-chat.html`
  - Drag a bot in the sidebar to reorder it, or drop it on a room.
  - Press **Approve** or **Deny**.
  - Toggle the browser panel with the panel button in the top bar.
  - Press **Take control**.
  - Open the **Forge menu** (the ellipsis button in the top bar) and choose **Replay Forge's turn**.
- `screens/desktop-agents.html`
  - Drag a bot card across the grid to reorder it, or onto a room stack to add it there.
  - Hover the cards and stacks.
  - Click a card to select it.
  - Use **More** then **Delete bot** to see the grid close ranks.
- `screens/phone-chat.html`: tap the microphone to dictate, or approve from the phone.
- `screens/phone-agents.html`: tap the host pill to open the sheet, or tap the bottom tabs.
- `components/motion.html`: every motion on its own, each with a Replay button and a reduced-motion preview switch.

Add `?still` to any URL to freeze the resting state, `?reduced` to preview reduced motion, or `?theme=dark` to switch theme.

## Files

| Path | What it is |
|---|---|
| `tokens.json`, `tokens.css` | Colour for both themes (including the 10 bot colours), type scale, spacing, radius, elevation and motion (durations, cubic-bezier easings, springs as stiffness and damping, plus a CSS `linear()` curve for each spring). |
| `components/*.html` | 24 cards. The first line of each is its `@dsCard` group. They link only `../tokens.css` and Google Fonts; the shared component CSS and motion script are inlined in each file. |
| `screens/` | `desktop-chat` (1440x900), `desktop-chat-alt` (the same screen in the dark theme), `desktop-agents`, `phone-chat` (412x892), `phone-agents`. |
| `shots/` | Headless Edge resting states: desktop at 1x, phone at 2x. |

Components by group:
- Foundations: colour, type, space and elevation, motion.
- Navigation: app shell, host switcher.
- Bots: avatar, status pill, bot card, room and project stacks, quota bar.
- Chat: message, thinking and tool rows, approval card, exchange card, composer.
- Browser: browser panel.
- Files: file card and preview.
- Overlays: dialog, phone sheet, toast.
- Forms: settings row, buttons and inputs.
- Projects: task card.

The HTML is generated from sources kept outside the repo, in `%LOCALAPPDATA%\Temp\tjommie-workbench-tools\`. That folder holds:
- `tokens.mjs`, the token source.
- `build.mjs`, which expands the avatar, icon and partial macros and builds the Lucide sprite from `node_modules/lucide-react`.
- `lint.mjs`, which checks the rules below.
- `shoot.mjs` (headless Edge) and `verify.mjs` (Playwright drags and script errors).

The files here are complete on their own and can be edited directly.

## Type

| Role | Family | Licence |
|---|---|---|
| Everything: UI, body, headings, bold page titles | **Hanken Grotesk** 400, 500, 600, 700 | SIL Open Font License 1.1 |
| Code, commands, paths | **JetBrains Mono** 400, 500 | SIL Open Font License 1.1 |

- The app bundles static TTFs of these weights; the HTML loads them from Google Fonts.
- Letter spacing is always 0, because GPUI has no letter spacing.
- Mono ligatures are off (`calt` off), so `->` stays two characters.
- Numbers use tabular figures (`tnum`) where they line up.

## Colour

- The chrome is monochrome: surfaces, text, ink buttons, rings and selection.
- Needs you is the only inverted pill (ink fill, light text), so it wins every scan.
- Working shows a spinner in the bot's own colour.
- The quiet states (idle, paused, limited) stay grey with an icon, and needs setup is outlined. State never depends on colour alone.
- Errors are the one non-content colour: a restrained red, always with an icon and words.
- `components/foundations-colour.html` computes every contrast pair live from `tokens.css`. All of them pass:
  - Body and muted text are at least 5.6:1 on every surface, in both themes.
  - Icon-only greys are at least 3.4:1.
  - Syntax colours on the code background are at least 4.8:1.

## Motion

### What the quiet.md clip shows

I stepped through the 12 s clip at 12, 15, 30 and 60 fps. The recording carries duplicate frames, so the timings below are good to about 33 ms. I also read the public quiet.md source (MIT, github.com/s1dashu/quiet-md, GPUI 0.2.2) to confirm the springs. The X post itself refused automated fetching.

- **Every motion is a damped spring ticked per frame.**
  - It uses semi-implicit Euler with 4 substeps, driven by `window.request_animation_frame()`, so it behaves the same at 60 and 120 Hz.
  - Layout uses stiffness 420 and damping 31 (ζ about 0.76).
  - Fades are mostly critically damped, for example stiffness 520 and damping 46.
- **Sidebar drag, 0.2 to 4.9 s.**
  - After a few pixels the row lifts into a white floating chip (about 8 px radius, soft shadow).
  - The chip is glued to the pointer with no lag.
  - The row's old slot closes, and a grey rounded ghost gap, one row tall, marks where it will land.
  - When the pointer crosses a row's midpoint, the gap jumps and the siblings slide to open it. Each slide takes about 150 to 200 ms, eases out and has no visible overshoot.
  - Over a folder, the folder icon swaps to an "open" glyph, and the folder and its children get a rounded grey drop wash. The gap then moves inside the folder.
  - Resting on a collapsed folder for 0.45 s opens it.
  - On release, the chip flies into the gap in about 100 to 130 ms and its shadow fades. The wash fades, and the new row shows its hover fill and its "..." action.
- **Row hover.**
  - The fill arrives within a frame or two.
  - "..." appears on the right of the row.
  - Folders swap their icon for a chevron and show "+" and "...".
- **Folder card hover, 5.0 to 6.0 s.**
  - The papers rise out of the folder one after another in about 200 to 270 ms. The front paper goes first; the back ones are on softer springs (stiffness 320, 230 and 170), so they trail.
  - The front pocket leans back.
  - "+" and "..." appear at the bottom right of the pocket.
  - Leaving reverses all of it.
- **Card into a folder, 6.0 to 7.5 s and 8.8 to 11.4 s.**
  - On pickup, the card instantly becomes a sketch paper with drawn lines instead of its content.
  - The paper shrinks to carry size (110 px wide) in about 100 ms. The card's slot becomes a flat grey ghost.
  - Over a folder, the papers part and fan out. The left ones tilt about −6° to −8° and the right one about +5°. This opens in about 130 to 170 ms and holds while you hover.
  - On release, the paper slides in behind the glass front and becomes the new front paper. It lands slightly tilted, then straightens.
  - At the same time the folder dips on a bouncy spring (stiffness 520, damping 15, ζ about 0.33), the fan settles over about 300 ms, and the count ticks up (for example 5 items becomes 6).
- **Grid reflow, about 9.9 s.** When the ghost is released, the next card snaps into the free slot within one captured frame. quiet.md does not slide its grid.
- **Selection ring.** The selected card has a crisp 2 to 3 px dark ring at its edge. It is static.

### Workbench's own motion

Workbench keeps the model (springs everywhere, papers that fan, sketches that shrink) and makes it its own:
- Bots join rooms rather than files going into folders.
- The stacks are plain paper with no glass pocket.
- The grid slides instead of snapping.
- Overlays never scale.

| Spring | Stiffness, damping | ζ | Settles in | Used for |
|---|---|---|---|---|
| `layout` | 420, 32 | 0.78 | 440 ms (looks done by about 220 ms) | Siblings sliding, grid reflow, the carried paper following the pointer, segmented and bottom-nav pills |
| `lift` | 520, 44 | 0.96 | 380 ms | Hover lift, actions dropping in, switch thumb, chevrons |
| `fan` / `fan-2` | 300, 22 / 230, 18 | 0.64 / 0.59 | 630 / 710 ms | Stack sheets parting and settling; the back sheet trails |
| `settle` | 260, 26 | 0.81 | 550 ms | Landings, the approval collapsing, toasts and sheets arriving |
| `dip` | 520, 16 | 0.35 | 840 ms (about 5 px of travel) | The stack dipping when it takes a paper |
| `panel` | 300, 34 | 0.98 | 500 ms | Browser panel width; no overshoot on big layout moves |
| `fade` | 520, 46 | 1.0 | 430 ms | Opacity that must not overshoot |

Plain curves:
- `ease-out` is cubic-bezier(0.23, 1, 0.32, 1), at 90 ms for a press, 140 ms for a hover, 180 ms for a fade and 220 ms for a reveal.
- `in-out` is used for the live pulse (1600 ms).
- `ease-in` is used for exits.

| Motion | Trigger | What moves | Meanwhile |
|---|---|---|---|
| Hover and press | Pointer over or down | Fill deepens in 140 ms, and again on press in 90 ms. Cards lift 2 px with a deeper shadow (`lift`). Card actions fade in (180 ms) and drop 4 px. | Nothing else moves |
| Tree reorder | Drag a bot row past 4 px | The row becomes a chip glued to the pointer. A ghost gap opens; siblings slide (`layout`). On release the chip springs into the gap (`settle`). | Alt+Up and Alt+Down move a focused row with no motion |
| Bot onto a room (tree) | Drop a row on a room | The room row fills. The chip shrinks to 44 px and fades into the row (`settle`, `fade`). The count ticks up. | The ghost gap returns home |
| Card carried as paper | Drag a bot card past 5 px | The card becomes a sketch paper. Its rect (x, y, width, height) springs toward the pointer and carry size, 118x132 (`layout`). A grey ghost holds its slot. | Over other cards the ghost moves and the grid reflows (`layout`) |
| Stack fans | Hover (level 1) or carry a card over it (level 2) | Two sheets rise and tilt, up to −8.5° and +7.8°, on `fan` and `fan-2`. The front card dips 5 px and gets the ring. | The ghost stays in its slot |
| Paper into a stack | Release over a stack | The paper flies to the mouth (`settle`), tucks behind the front (70 px down, fading), then the stack dips and settles. The bot's avatar slides into the member row. | The card fades back into its slot; a toast rises |
| Card leaves | Delete bot | The card fades and sinks 8 px (`fade`, `settle`), then the following cards close ranks (`layout`). | A toast confirms |
| Browser panel | Panel button or Hide | Panel width springs between 560 and 0 (`panel`). The panel keeps its own width and slides past the window edge; the chat column widens with it. | Content opacity follows the width |
| Streaming turn | A bot answers | Tool rows grow from 0 height (`layout`) with a spinner in the bot's colour, then settle: the spinner cross-fades to a check (90 ms out, 180 ms in) and the text quiets. Words fade in over 120 ms as they arrive. | The thread follows the newest line on `layout` |
| Approval resolves | Approve or Deny | The contents fade in 120 ms. The height springs from about 96 px to one 30 px line; padding and the lifted shadow sink into the page (`settle`). | What follows slides up; the composer hint switches to steer or queue |
| Live marks | Only while live | The live dot pulses 1 to 0.32 opacity (1600 ms, in-out). The working arc turns (900 ms, linear). Dictation meter bars follow the microphone level (`lift`). | Everything stops dead when it is no longer live |
| Toast, dialog, sheet | Show or hide | Opacity plus travel: a toast rises 14 px, a dialog 10 px, a sheet its full height (`settle`, `lift`). The scrim fades in 180 ms. Exits use `ease-in` (180 ms). | Never scaled, because GPUI cannot scale text |

- **Keyboard actions never animate.** The prototypes track the last input. Tab, Enter, Space and Alt+arrow snap.
- **Reduced motion.** Moves snap and appearances cross-fade (opacity, 180 ms). The live dot holds steady and spinners stop; the words still say Working or Live.
- **Interruptible.** A new target retargets the running spring from its current position and velocity.

## Rendering table

Every visual effect in these files has both a GPUI and a Jetpack Compose primitive. Anything without both was removed.

| Effect | Where | GPUI | Compose |
|---|---|---|---|
| Solid fills | All surfaces, pills, buttons | `.bg()` | `Modifier.background` |
| Per-corner radii | Cards 18, papers 16, rows 9, pills full | `.rounded_*()` / `corner_radii` | `RoundedCornerShape` |
| 1 px solid borders | Dark-theme card edges, hairlines, paper edges, the outlined pill | `.border_1().border_color()` | `Modifier.border` |
| Soft multi-layer shadows | Card, hover, lift, pop, drag, chip | `.shadow(vec![BoxShadow { color, offset, blur_radius, spread_radius }])` | `Modifier.dropShadow(shape, Shadow(...))` (Compose UI 1.9 and later, BOM 2025.08). Older Compose: `Modifier.shadow(elevation, shape, ambientColor, spotColor)`, with colours from API 28. |
| Rings (focus, selection, drop target) | A 2 px gap plus a 2 px ink ring, or a 2 px ring at the edge | `BoxShadow` with `spread_radius` and zero blur (GPUI has no outline offset) | `Modifier.border` plus padding for the gap, or a `drawBehind` stroke |
| Opacity | Paused and limited avatars, fades | `.opacity()` | `Modifier.alpha` / `graphicsLayer { alpha }` |
| Two-stop linear gradient | Fade at the bottom of chat miniatures, right edge of phone code | `linear_gradient(angle, from, to)` | `Brush.verticalGradient` / `horizontalGradient` |
| Lucide icons, one family | Everywhere | `svg().path(..).text_color(..)`, a single-colour mask | `ImageVector` in `Icon(tint = ..)` |
| Bot avatars: the exact 8 bodies and eyes | Tree, cards, chat, phone | Two stacked `svg()` layers (body tinted with the bot colour, eyes tinted white), because `svg()` is a monochrome mask | One `ImageVector` with two paths (fill and white stroke) |
| Spinner arc | Working | `svg().with_animation(.., |s, d| s.with_transformation(Transformation::rotate(percentage(d))))` (Zed's spinner) | `Canvas` arc with `rememberInfiniteTransition` rotation |
| Images with their own radii | Browser stream frame, thumbnails | `img()` with corner radii | `Image(bitmap)` with `Modifier.clip(RoundedCornerShape)` |
| Rectangular clipping only | Miniatures, panel sliding out, approval collapsing | `overflow_hidden()`: GPUI masks are rectangles, so rounded containers keep their content inset | `Modifier.clipToBounds()` (any shape works) |
| Tilted sheets | Stack papers, exchange sheets on hover, the landing paper | Painted paths in `canvas()` with `PathBuilder` (rotated rounded rects, as quiet.md's `home_folder.rs` does), or `svg()` with `Transformation::rotate`. Text never rotates. Rotated shapes get a 1 px edge and a flat 1 px offset, never a blurred shadow. | `graphicsLayer { rotationZ }` |
| The carried paper shrinking | Card pickup | Interpolated width and height, children placed in percent; no transform | Animated `Modifier.size` or `graphicsLayer { scaleX; scaleY }` |
| Translations | FLIP slides, lifts, sheet travel | Offsets updated per frame (`.relative().top(px(..))` / absolute `left`, `top`) | `Modifier.offset { }` / `graphicsLayer { translationX; translationY }` |
| Truncation and line clamp | Tabs, rows, miniatures | `.truncate()`, `line_clamp(n)` | `maxLines` with `TextOverflow.Ellipsis` |
| Font features | Tabular numbers, no mono ligatures | `FontFeatures` (`tnum` on, `calt` off) | `fontFeatureSettings` |
| Scrim | Dialog, sheet | A fill with alpha | `Box` with a fill alpha / `ModalBottomSheet` scrim |
| Own title bar and caption buttons | Desktop top bar | A custom title bar with window control areas for minimise, maximise and close | Not applicable |

Removed or never used:
- Blur behind, CSS filters and blend modes.
- Radial and conic gradients, text clipping, noise.
- Dashed borders: today's dashed New bot tile became a top-bar button.
- Outline offset and letter spacing.
- Inset shadows: the specimen frames use a border.
- Scaling anything that carries text: popovers, dialogs and toasts fade and travel instead.
- Rotating anything with text: only blank sheets and sketches tilt, and the incoming paper is a sketch until it settles.

`lint.mjs` checks all of this on every build, along with:
- The `@dsCard` first lines.
- No raw colours outside `tokens.css`.
- Every `var()` resolves to a token.
- No em or en dashes, emoji or placeholder copy in visible text.

## Motion in native code

| Motion | GPUI | Compose |
|---|---|---|
| All springs | A small `Spring { p, v, target, k, c }` stepped in `request_animation_frame` (4 substeps). Its values feed layout (offsets, widths, heights), opacity and `BoxShadow`. It retargets by changing `target`. One-shots can use `with_animation(id, Animation::new(d).with_easing(spring_curve), ..)`. | `spring(dampingRatio = c / (2 * sqrt(k)), stiffness = k)` with `animate*AsState`, or `Animatable.animateTo` for retargeting with velocity |
| Hover, press, lift, actions | A per-element `Fade` (quiet.md uses stiffness 1100, damping 66 for hover) | `interactionSource` with `animateColorAsState`, `animateDpAsState`, `AnimatedVisibility(fadeIn() + slideInVertically())` |
| Tree reorder and gap | `on_drag` with a chip preview view, `drag_over` styling, per-row offset springs | `LazyColumn` with `Modifier.animateItem(placementSpec = spring(..))`, and drag via `detectDragGesturesAfterLongPress` or `dragAndDropSource` / `dragAndDropTarget` |
| Carried paper and grid reflow | An overlay element with sprung bounds; per-card offset springs | An overlay `Box` with animated offset and size; `LazyVerticalGrid` with `animateItem()` |
| Stack fan and dip | `canvas()` paths for the sheets with one `Fade` per sheet (exactly quiet.md's approach) | `graphicsLayer { rotationZ; translationX; translationY }` per sheet with spring states |
| Panel width | `div().w(px(sprung))` with `overflow_hidden` | `animateDpAsState(spring)` on width with `clipToBounds`, or `AnimatedVisibility(expandHorizontally() / shrinkHorizontally())` |
| Approval collapse, rows growing | Sprung `h()`, padding and shadow | `animateContentSize(spring)` / `AnimatedContent` with `SizeTransform` |
| Streaming words | Per-chunk opacity keyed by message id and chunk index | Per-chunk alpha with `Animatable` |
| Live pulse, spinner | `with_animation(.., Animation::new(1.6s).repeat().with_easing(pulsating_between(0.32, 1.0)), ..)`; spinner as in the rendering table | `rememberInfiniteTransition().animateFloat` |
| Toast, dialog, sheet | A `deferred()` overlay with sprung offset and opacity | `AnimatedVisibility(slideInVertically() + fadeIn())`, `ModalBottomSheet` |

### Phase 2 spike checks (uncertain, measure on the owner's PC)

1. **Drop landing.** GPUI's built-in drag preview disappears on release, so the landing flight needs our own overlay handed the position on the same frame. Check for a one-frame gap.
2. **Many live springs.** Run the springs for a 10-card grid plus a 30-row tree and confirm p95 frame time is 16.7 ms or less. Confirm the loop stops requesting frames once everything settles.
3. **Painted sheets.** Check anti-aliasing at ±9° tilt, that hit-testing stays on the upright front card, and that the sheets look right without blurred shadows.
4. **Animated shadows.** Check the cost of interpolating `BoxShadow` blur and offset for the lift and drag shadows.
5. **Caption buttons.** Check that Windows snap layouts appear on maximise hover through GPUI's window control areas.
6. **Fast streams.** Check per-chunk fades at about 60 chunks per second.
7. **Compose shadows.** `Modifier.dropShadow` needs Compose UI 1.9. Pin a BOM that has it, or fall back to `Modifier.shadow`.

## Inspiration, not a copy

quiet.md (MIT) inspired the surfaces, the full-height sidebar tree, the shared top bar with a centred command field, live miniature cards, papers in stacks and the spring motion. No layout, copy, icons, assets or frames were copied, and none of its frames are in this repo.

The differences:
- Stacks are paper with no glass folder.
- The content is Tjommie's own bots, rooms and projects.
- The icons are Lucide.
- The type is Hanken Grotesk.
- The grid slides where quiet.md snaps.
