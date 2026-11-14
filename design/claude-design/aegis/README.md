# Aegis

A design direction for **Tjommie**, in the brand of the owner's company, Aegis. The app is called Tjommie everywhere it names itself; "Aegis" is the name of this direction and its folder.

## Feel it first

Open the screen files in Chrome or Edge to feel the motion. They are live prototypes, not pictures:

- `screens/desktop-chat.html`: Forge's turn streams in on load. Approve or Deny the card, type in the composer, open the model pill, dictate, send, close and reopen the browser panel, and try **Scout needs you** in the prototype bar.
- `screens/desktop-agents.html`: approve Forge from its Needs you card and watch the numbers follow. Open chat flies Forge's avatar into the chat.
- `screens/phone-agents.html` (it renders at phone size in the top left; use the mouse as a finger): the first open is staged once, the drawer pulls from the left edge or the menu button, Review opens a sheet you can drag, and tapping Forge flies it into its chat.
- `screens/phone-chat.html`, `screens/phone-drawer.html`, `screens/desktop-chat-alt.html` (dark).
- `components/motion.html`: every motion on its own card with **Replay**.

The prototype bar (top of desktop screens, right of phone screens) switches **Dark** and **Reduced motion** and replays. Add `?shot` to any screen URL to see the static resting state (that is how the PNGs in `shots/` are made), or `?noplay` to skip the turn that plays on load. Short recordings of each signature moment are in `motion-clips/`.

## The idea

A quiet stone workspace where the agents are the only colour, and the only things that seem alive. The chrome is the measured Aegis brand, monochrome stone and ink: a stone frame, paper panels, white raised cards with the brand's glass shadow, near-black pill buttons and underlined text actions. The seven agents bring the colour, in one even lightness band, so they read as a team rather than a rainbow. One ochre accent is reserved for one meaning: something is waiting for you. On the phone, a drawer holds the wordmark, the sections and a Recents list; on the desktop the same drawer is a persistent sidebar. A large white composer card is pinned to the bottom of every chat, with the agent's provider, model and effort in a stone pill. Motion is calm, weighty and sculptural: big surfaces arrive on a critically damped spring and settle like stone set down well, small parts land with a short settle, and the agents blink, look up and focus. The dark theme is the same palette in warm graphite.

## Owner decisions recorded

- **App name:** Tjommie. The wordmark is "Tjommie" in Archivo Expanded ExtraBold (28 px in the sidebar, 30 dp in the drawer; both fit with room to spare).
- **Amber** stays light (`#E2A72E`), owner-approved as the one contrast exception. Amber avatars always appear with the agent's name.
- **The stone-shield mark** (a sculpted shield wearing the agents' two eye strokes) is owner-approved.

## Files

| Path | What it is |
|---|---|
| `tokens.json` | Colour (light, dark, 10 agent colours), type (desktop and phone), space, radius, size, shadow, and motion: durations, easings, springs, stagger, travel, distances, eyes, sweep. DTCG style; px on desktop equals dp on Android. |
| `tokens.css` | The same tokens as CSS custom properties, including each spring as a ready-made `linear()` curve (`--spring-calm: 560ms linear(...)`). `[data-theme="dark"]` switches theme, `[data-device="phone"]` switches the type scale, `[data-motion="reduced"]` (and `prefers-reduced-motion`) zeroes every motion distance. |
| `components/*.html` | One self-contained card per group, first line `@dsCard`. `motion.html` (Foundations) isolates each motion with Replay. |
| `screens/*.html` | Live prototypes with JS, both themes: `desktop-chat`, `desktop-chat-alt` (dark), `desktop-agents`, `phone-chat`, `phone-agents`, `phone-drawer`. |
| `shots/*.png` | The resting state, rendered with `?shot`: desktop at 1x (1440 by 900), phone at 2x (412 by 892). |
| `motion-clips/*.webm` | Recordings of the signature moments, made from the live HTML with Playwright. |

## Motion

### Character and rules

Calm, weighty, sculptural. Every motion:

- starts on the frame of the input, and can be caught and turned mid-flight (tweens retarget from where they are; drags hand their velocity to the spring);
- never plays for a keyboard action (Enter sends at once, Escape closes at once, a key-triggered click snaps); typing is not a command, so the agent's glance at what you type still moves;
- never scales or rotates text: a pressed label stays put while its fill compresses, cards and toasts only translate and fade, menus grow their surface rather than their words;
- under reduced motion becomes a cross-fade or a snap: distances go to zero, shared elements cross-fade, blinks, scans, glances and the mark's light stop, numbers snap.

### Tokens

| Spring (mass 1) | Stiffness | Damping ratio | Settles | Used for |
|---|---|---|---|---|
| calm | 260 | 1.0 (no overshoot) | 560 ms | drawer, sheets, panels, cards, rows, shared elements, eyes opening |
| snappy | 520 | 0.8 (overshoots 1.5%) | 360 ms | chips, toggles, the send button, menus, glances, tool rows |
| playful | 380 | 0.7 (overshoots 4.6%) | 510 ms | the Approve stamp, toast arrival, rolling digits, the mark's entrance |

| Duration | ms | | Easing | Curve |
|---|---|---|---|---|
| press / release | 140 / 240 | | out (brand) | `cubic-bezier(.23, 1, .32, 1)`: everything the user starts |
| hover | 200 | | move | `cubic-bezier(.45, 0, .15, 1)`: things the system moves on its own (a Recents row reordering) |
| fade / quick / base | 160 / 200 / 280 | | pulse | `cubic-bezier(.37, 0, .63, 1)`: the live dot and the eye scan |
| settle / slow / entrance | 360 / 420 / 460 | | linear | the tool spinner |

Travel time for anything that crosses the screen: 280 ms plus 0.45 ms per px, capped at 450 ms (74 px: 313 ms; 336 px: 431 ms). Stagger: 40 ms (tight), 70 ms (step), 90 ms (loose). Press fill 94%, icon press 94%, lift 2 px, rise 8 px, enter 6 px, page recede 97% and 24 px. Flick: 0.4 px per ms. Eyes: blink every 2.6 to 6.4 s (close 70 ms, open 110 ms, sometimes twice), scan 1.8 s, glance held 1.2 s. Mark sweep: 1.7 s, every 4.6 s while any agent works.

### Signature moments

1. **Agents feel alive.** The eyes are their own layer (`look`, `scan`, `lid`). Each agent blinks on its own clock. A working agent's eyes narrow and scan side to side instead of a spinner (spinners remain only for tools and connections). An agent that needs you looks up at you; a limited agent is drowsy and blinks slowly; a paused agent rests with its eyes shut. While you type to an agent, its eyes glance at the composer. The poses are static, so a still or reduced-motion frame still reads.
2. **Approve.** The pill's fill compresses on press. The card stamps down 3 px on the playful spring while its edge flashes ochre and settles to ink. After a 260 ms beat it closes into its one-line record (calm spring), the check draws on, and the followers move one stagger step apart: the header state, the Recents marker, the composer hint, then the next tool row ticks in. Deny skips the stamp and lets go quietly.
3. **Message arrival.** Tool rows tick in one after another on the snappy spring and their checks draw on as each result lands. Words fade in one at a time, 34 ms apart, never letter by letter. The code block grows to its height on the calm spring while its lines settle 40 ms apart. The thread is bottom-anchored, so earlier content glides up rather than jumping.
4. **Composer.** On focus the card lifts 2 px and grows. The model pill's own surface grows into the menu (bounds, radius, fill, shadow on the snappy spring; choices fade in 90 ms later) and shrinks back into the pill. Dictation drives each bar of the level meter with its own snappy spring; the words land in the draft and are never sent on their own. The send button wakes with a snappy pop when there is something to send. Send flies the draft up into the timeline as the new message while the thread makes room (FLIP).
5. **Phone drawer.** It follows the finger 1:1, from the menu button or the left 24 px of the screen; past its open edge it rubber-bands; on release a flick decides by direction and a slow release by position, and the calm spring carries the gesture's velocity. The page behind eases back (24 px, 97%) and dims on the same progress value.
6. **Numbers.** Spend, quota percentages and Needs you counts roll between values: tabular digits, rightmost first, 30 ms apart, up when the number grows, on the playful spring. Bars ease to their new length.
7. **The stone mark.** At rest it blinks like the agents. While any agent is working, a slow band of light passes over the stone every 4.6 s: one linear gradient, moved across.
8. **First open of the phone home.** Once per device (the prototype remembers it in the browser; Replay first open plays it again): the mark settles first (playful), then the greeting, the summary, the Needs you card and the list follow one step apart. Later visits open straight to the content.

Shared elements across screens: from the agents list (desktop card, Recents row, phone row), Forge's avatar flies into the chat header; on the phone, the Needs you item flies in and becomes the approval card. Back on the phone, the avatar flies home into its row, and if Forge was approved in the chat its item leaves the card while the counts roll.

### Clips

`motion-clips/`: `01-bots-alive.webm`, `02-approve.webm`, `03-message-arrival.webm`, `04-composer.webm`, `05-phone-drawer.webm`, `06-numbers.webm`, `07-mark.webm`, `08-first-open.webm`, `09-shared-elements.webm`, `10-browser-panel.webm`.

## Brand values used

Light is the Aegis site, measured: bg `#E6E6E3`, paper `#F2F2EF`, white `#FFFFFF`, ink `#191A1C`, ink-mid `#4A4C50`, ink-muted `#6B6D70`, line `#D6D6D1`, line-soft `#C4C4BF`, accent `#B57A1F`, glass shadow `0 24px 60px #3C3C3A2E` with a light top edge, easing `cubic-bezier(.23, 1, .32, 1)`.

Four values are derived, each for contrast:

| Token | Value | Why |
|---|---|---|
| `text-3` | `#5F6164` | ink-muted is 4.1:1 on bg; text on the stone frame needs 4.5:1. ink-muted stays for icons. |
| `marker` | `#AC711F` | The ochre dot. The brand ochre is 2.9:1 on bg; a state dot needs 3:1. |
| `accent-text` | `#895709` | Ochre words ("Needs you") at 5.1:1 on the accent tint. |
| `line-field` | `#838588` | Text-field edges at 3:1, so a field is visible as a field. |

Dark theme (warm graphite) is derived in OKLCH at the brand greys' hue: bg `#141412`, paper `#1D1C1A`, raised `#272724`, ink `#EEEEEB`, ochre lifted to `#DDA351`.

## Contrast

Every text token clears 4.5:1 on every surface it is used on, in both themes, and every glyph, state dot and field edge clears 3:1. The ratios are computed from the tokens, and `foundations-color.html` prints them. The agent colours are retuned into one lightness band; each clears 3:1 on paper and on graphite and keeps its white eyes at 3:1, except **amber** (`#E2A72E`, 1.9:1 on paper, eyes 2.1:1), which stays light so it is never mistaken for the ochre accent. That exception is owner-approved, and an amber avatar always appears with its agent's name.

## Rendering table

Every visual effect in the HTML, mapped to what GPUI and Jetpack Compose draw. Nothing without both was used.

| Effect (where) | GPUI | Jetpack Compose |
|---|---|---|
| Solid fills: frame, panels, cards, pills, bars | `div().bg(rgb)` | `Modifier.background(color, shape)` |
| Translucent fills: hover 5%, pressed 9%, scrim 38% (dark 55%) | `hsla(h, s, l, a)` fill | `Color(...).copy(alpha = a)` |
| Uniform radii and pills (6 to 32, full) | `.rounded(px(n))`, `.rounded_full()` | `RoundedCornerShape(n.dp)`, `CircleShape` |
| Per-corner radii: your bubble (20, 20, 6, 20), drawer (0, 28, 28, 0), sheet (28, 28, 0, 0) | `.rounded_tl()` / `.rounded_tr()` / `.rounded_br()` / `.rounded_bl()` | `RoundedCornerShape(topStart, topEnd, bottomEnd, bottomStart)` |
| 1 px borders: dividers, field edge, quiet pill outline, toggle track | `.border_1().border_color(c)` | `Modifier.border(1.dp, c, shape)` |
| Glass top edge, replacing the site's `inset 0 1px 0 #fffc` | top border only: `.border_t_1().border_color(highlight)` | 1 dp stroke along the top edge in `Modifier.drawWithContent`, or `Modifier.border` with a vertical brush that fades out after 1 dp |
| Drop shadows: glass `0 24 60`, raised, card, card-lift, raised-focus; dark variants | `.shadow(...)` with `BoxShadow { color, offset, blur_radius, spread_radius }` | `Modifier.dropShadow(shape, Shadow(radius, spread, color, offset))` (Compose UI 1.9 and later) |
| Linear gradient: the mark's stone fill; the code fade on phone | mark: `img()` of the SVG; fade: `.bg(linear_gradient(90deg, stop, stop))` | mark: `ImageVector` path with `Brush.linearGradient`; fade: `Brush.horizontalGradient` |
| Opacity: disabled 38%, empty send | `.opacity(0.38)` | `Modifier.alpha(0.38f)` |
| Agent avatars: exact body and eye paths from `bot-avatar.tsx`, 40 by 40 box | two `svg()` layers: body tinted the agent colour, eyes tinted white (`svg()` is a single-colour mask) | `ImageVector` body plus a separately drawn eye path |
| Icons: Lucide, stroke 1.75 (caption buttons 1.25) | `svg().path("icons/<name>.svg").text_color(c)` | Lucide paths as `ImageVector`, `Icon(tint = c)` |
| State badge cut out of the surface | absolute child, `.border_2().border_color(surface)`, `.rounded_full()` | `Box` + `Modifier.align(BottomEnd).offset(..)`, `border(2.dp, surface, CircleShape)` |
| Focus ring: 2 px ink, 2 px gap | wrapper with `.border_2()` outside a 2 px padding when `focus_visible` | `Modifier.padding(2.dp).border(2.dp, focus, shape)` from `collectIsFocusedAsState()` |
| Underlined text actions | `.border_b_1()` on the text element | `Modifier.drawBehind { drawLine(...) }` |
| Ellipsis truncation | `.truncate()` | `maxLines = 1, overflow = TextOverflow.Ellipsis` |
| Display face: Archivo width 125, weight 800, tracking -0.02 em | bundle static **Archivo Expanded ExtraBold**, advances tightened by 0.02 em (fonttools): GPUI sets neither variable axes nor letter spacing | the same static font plus `letterSpacing = (-0.02).em` |
| Text and mono faces | bundled TTFs via `cx.text_system().add_fonts()` | `FontFamily(Font(R.font.archivo_regular), ...)` |
| Browser page and live frames | `img()` of the current frame | `Image(bitmap = frame)` |

### Motion

GPUI animates with `with_animation(id, Animation::new(duration).with_easing(f))`; the easing is any `Fn(f32) -> f32`, so each spring becomes its closed-form step response over its settle time (the same function that produced the `linear()` curves in `tokens.css`). Compose uses `spring(dampingRatio, stiffness)` directly. **Spike** marks what Phase 2 must prove before it is promised.

| Motion | GPUI | Compose | Spike |
|---|---|---|---|
| Fixed-target springs (cards, rows, toasts, menus, stamp) | `Animation` with the spring curve as easing | `spring(...)` in `animate*AsState` / `Animatable` | **Yes:** retargeting mid-flight with velocity needs our own state (a GPUI animation restarts from its start value) |
| Press: the fill compresses, the label stays | the fill is its own absolute child; its inset is interpolated, the label is untouched. Icon buttons: `svg().with_transformation(Transformation::scale(..))` | `graphicsLayer { scaleX; scaleY }` on the background only, from `collectIsPressedAsState()` | No |
| Hover lift: 2 px up, deeper shadow | `.relative().top(..)` and the `BoxShadow` fields interpolated per frame | `animateDpAsState` offset, `dropShadow` parameters animated | No |
| Living eyes: blink (scaleY), look (translate), working scan, poses | eyes layer `svg().with_transformation(Transformation::scale(size(1., s)).with_translation(point(dx, dy)))` driven by `with_animation`; random blink timers per visible avatar with `cx.spawn` | eye path drawn in `Canvas` inside `withTransform { scale(1f, s, pivot = eyeCentre); translate(dx, dy) }` | **Yes:** confirm the transformation pivot (eyes must scale about their own centre, so the eye asset may need centring on the eyes) and the cost of 30 or more live avatars |
| Rolling digits | each digit an `.overflow_hidden()` cell; the two stacked digits translate (text only translates) | `AnimatedContent(digit)` with `slideInVertically` / `slideOutVertically` | No |
| Words fade in one at a time | one `StyledText` with a per-run alpha, advanced per frame (no per-word movement, so the HTML fades only) | `AnnotatedString` with per-word `SpanStyle` alpha | **Yes:** per-run alpha updates at 30 per second on long messages |
| Check draws on | `canvas()` with `PathBuilder::stroke` of the check path cut to the progress length | `PathMeasure.getSegment(0f, length * p, dst)` in `Canvas` | **Yes:** partial path stroking in GPUI |
| Code block grows to its height; composer grows; card closes into its record | height (and padding, fill, shadow) interpolated per frame with `.overflow_hidden()` | `animateContentSize(spring(...))`, `AnimatedContent` with `SizeTransform` | No |
| Model pill becomes its menu | an anchored overlay at the pill's bounds whose size, radius, fill and shadow are interpolated; content fades in after | `SharedTransitionLayout` + `sharedBounds()` between pill and menu | **Yes:** overlay layering over the composer, bounds capture |
| Send: the draft flies into the timeline | a temporary overlay of the text translated from the field's bounds to the new message's bounds | `Modifier.sharedElement` between the draft and the new bubble | **Yes:** cross-element bounds in GPUI |
| Shared avatar between screens; Needs you item into its chat | overlay avatar animated between captured bounds of the two views in one window | Navigation Compose + `SharedTransitionLayout` + `sharedElement(rememberSharedContentState(key = "bot-forge"))` | **Yes:** GPUI view-to-view bounds and overlay timing |
| Recents reorder (FLIP) | keep each row's previous bounds, paint with an interpolated offset toward the new layout | `LazyColumn` + `Modifier.animateItem(placementSpec = spring(...))` | **Yes:** previous-bounds capture in GPUI lists |
| Item leaves and the list reflows | row height and opacity interpolated, then removed | `animateItem(fadeOutSpec, placementSpec)` or `AnimatedVisibility(shrinkVertically() + fadeOut())` | No |
| Bottom-anchored thread glides as it grows | bottom-aligned list with its scroll offset animated toward the new end | `LazyColumn(reverseLayout = true)` + `animateItem()` | **Yes:** smooth end-anchoring in a GPUI list |
| Drawer and sheet: drag 1:1, rubber band, release with velocity, page recedes and dims | phone only | `AnchoredDraggableState` with a `spring(1f, 260f)` snap spec, velocity threshold 400 dp per second, resistance past the anchors; the page behind in `graphicsLayer` from the same offset | No |
| Toast drops in with a settle | overlay offset and opacity, playful curve | `AnimatedVisibility(slideInVertically(spring(0.7f, 380f)) + fadeIn())` | No |
| The mark's light sweep | the shield path painted twice: stone, then a moving gradient band | `drawPath(path, Brush.linearGradient(start + o, end + o))` with an infinite transition | **Yes:** gradient fill on an arbitrary path in GPUI (`paint_path` with a gradient `Background`) |
| Dictation meter: one spring per bar | bar heights stepped by a spring each frame (`window.request_animation_frame`) | `Animatable(level).animateTo(target, spring(0.8f, 520f))` per bar | No |
| Browser panel opens and closes while the chat resizes | panel width interpolated; content revealed, not squashed | phone has no panel | **Yes:** the WebView2 child window must follow per frame, or freeze to a frame snapshot during the move |
| Staged first open, followers on a stagger | each element's animation offsets its own start inside the easing closure (GPUI has no delay) | `LaunchedEffect` + `delay(step * i)`, `AnimatedVisibility` with the spring | No |
| Tool spinner, live pulse | `svg().with_animation(.., Transformation::rotate(..))`; opacity loop | `rememberInfiniteTransition()` | No |

Not used anywhere: backdrop blur, CSS `filter`, blend modes, conic or radial gradients, background-clipped text, inset shadows, text shadows, noise or grain, dashed borders, images of stone, scaled or rotated text. The build script rejects raw colours, dashes in visible text and the unsupported effects.

## Fonts

| Family | Use | Weights bundled | Licence |
|---|---|---|---|
| Archivo (Omnibus-Type) | Interface and body text, width 100 | Regular 400, Medium 500, SemiBold 600 | SIL Open Font License 1.1 |
| Archivo Expanded (static width instance of Archivo) | Display: names, titles, the Tjommie wordmark | ExtraBold 800, advances tightened 0.02 em for GPUI | SIL Open Font License 1.1 |
| JetBrains Mono (JetBrains) | Labels, metadata, numbers, paths, code | Regular 400, Medium 500, SemiBold 600 (code keywords) | SIL Open Font License 1.1 |

Icons are Lucide (ISC licence), one family, drawn inline. The HTML loads the fonts from Google Fonts; the apps bundle the static files.

## Navigation: the drawer replaces the bottom bar

The phone has no bottom navigation. Every destination stays reachable:

- **Agents, Rooms, Projects, Routines, Settings:** drawer rows (the menu button at the top left of top-level screens, or a drag from the left edge).
- **Any agent or room:** drawer Recents and View all, or the Agents and Rooms lists.
- **New agent:** the near-black pill at the foot of the drawer (and the sidebar on desktop).
- **Switch PC:** the host pill on Agents (a bottom sheet listing This PC and Studio PC). Pairing lives under Settings, Devices. On desktop, the avatar at the foot of the sidebar opens the same list.
- **An agent's live browser:** the globe button in its chat app bar, which opens the live view.
- **Back:** Android back closes the drawer or sheet first, then walks back.

## Inspiration, and what is not borrowed

Taken from the mobile assistant app the owner pointed to: a drawer as the phone's navigation with a wordmark, outline-icon rows, a Recents list and a pill at its foot; a calm home with a centred mark and one greeting line; the composer as a large rounded card with round controls and a model pill. Not taken: its wordmark, its mark, its serif, its layout measurements or its copy. The wordmark is Tjommie in Archivo Expanded ExtraBold. The centred mark is the owner-approved stone shield. No screenshots or site images are copied into this folder.

## Scenario notes

The shared scenario is used word for word. Additions are marked as examples in the component cards. Choices where a reader might expect something else:

- The phone composer has no separate @ button; + opens Attach and Mention an agent, and typing @ mentions. The desktop composer has all four controls.
- The send button is dimmed in the resting screens because the field is empty ("Message Forge" is the placeholder).
- Per-agent spend on the agents overview (Forge $1.64, Scout $1.21, Quill $0.87, Ledger $0.40) adds up to the scenario's $4.12; approving Forge in the prototype moves it to $1.71 and the total to $4.19.
- Toasts appear at the top centre under the header on every screen, so they never cover the newest content at the bottom of a thread.
- In the docked browser panel, Take control and Return to agent are raised white pills, so Approve stays the one near-black action beside the chat. The phone's live view keeps Take control near-black.
