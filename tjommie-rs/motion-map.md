# Motion map (Phase 3 plan T7.1)

Every card in `design/claude-design/aegis/components/motion.html` (numbered as there, 00 to 12, plus the README's shared
elements) and every motion inside it: its tokens (`design/themes/aegis.json` `motion.*`), the type that implements it,
and the check that proves it. A motion the desktop has no place for says "n/a on desktop" and why; one that waits for
0.6.1 says "0.6.1: deferred by owner decision (2026-11-22)", with what the app does meanwhile, and is listed again at the end.

The rules every motion keeps (plan T2.3, T7.2):

- **Keyboard:** a change a key makes lands at once, with no frames in between (`motion::keyboard::Trigger`). Typing is
  no command, so the glance at what you type and the send button's pop still move.
- **Reduced motion:** with Windows' Animation effects off (or `--force-reduced`), every motion lands at once: no frames
  in between, the eyes keep their pose, numbers snap (`motion::reduced`, which also drives GPUI's and gpui-kit's own
  flag). The design's reduced-motion fades and cross-fades are snaps here too (the plan's rule wins).
- **Interruption:** springs keep their velocity when retargeted (`motion::spring::Spring::set_target`), so a motion
  caught mid-flight turns and never jumps.
- **Frames:** the motion clock (`motion::clock`) asks for frames only while something moves, at 30 fps in an inactive
  window unless something live shows (`Live`), and logs each frame for the motion checks.

Checks named `scenario:` are the app's own motion checks (T7.2, `tjommie-desktop --scenario <name>` in a test-driver
build; `tools/perf/run-motion-checks.ps1` runs them under PresentMon). The rest are unit or view tests in
`crates/desktop`.

## 00 The springs and the clock

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| calm, snappy, playful springs | `spring.calm` (260, 1.0), `spring.snappy` (520, 0.8), `spring.playful` (380, 0.7) | `motion::spring::Spring` (semi-implicit Euler, 8 substeps; `Spring::scaled` for fractions) | `spring::calm_settles_in_560ms_within_tolerance`, `snappy_and_playful_overshoot_as_the_tokens_say`, `retarget_keeps_velocity`, `a_fraction_lands_without_a_jump`, `snap_completes_in_one_step` |
| ease out, move, pulse curves | `easing.out`, `easing.move`, `easing.pulse` | `motion::eyes::bezier` | `eyes::eyes_follow_the_tokens` |
| the clock and its throttle | `duration.*` | `motion::clock` (`keep_moving`, `wake_in`, `Live`, the frame log, `--trace`) | `clock::inactive_throttles_unless_live`, `a_redraw_mid_interval_starts_no_second_chain`, `the_frame_ends_after_the_overlays_paint`, `logs_the_last_frames_and_traces_them` |
| reduced motion follows Windows | | `motion::reduced` | `reduced::follows_setting_change`, `gpui_animations_and_the_sheet_follow_a_live_change`; scenario `motion-reduced` |
| a key never animates | | `motion::keyboard::Trigger`, `motion::keyed::spring` | `keyboard::snaps`; scenario `motion-keyboard` |
| travel and stagger | `travel.*`, `stagger.*` | not used: every desktop motion rides a spring (see 11) | n/a |

## 01 Agents feel alive

One set of eyes per agent (`motion::eyes::Living`), so all of an agent's avatars blink, scan and glance together. Living
avatars: the Recents rows, the Agents and Needs you cards, the chat header and the empty chat. Message meta lines and
pickers keep still poses.

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| blink, now and then twice | `eyes.blinkClose` 70, `eyes.blinkOpen` 110, `eyes.blinkMin` 2600, `eyes.blinkMax` 6400, `easing.out` | `motion::eyes::{Eyes, blink, living}`, `BotAvatar::living` | `eyes::eyes_follow_the_tokens`, `every_avatar_of_an_agent_shares_its_eyes_and_agents_blink_apart`; scenario `motion-alive` |
| working: lids narrow, eyes scan | `eyes.scan` 1800, `easing.pulse` | `motion::eyes::scan` | `eyes::a_working_agent_scans_and_glances_at_the_composer`, `a_living_avatar_asks_for_frames_only_while_its_eyes_move` |
| glance at the composer while you type | `eyes.glanceHold` 1200 | `motion::eyes::glance` (the composer's text changes) | `eyes::typing_makes_the_agent_glance_down_at_the_composer` |
| needs you: looks up; limited: drowsy; paused: rests | poses | `ui::primitives::avatar::{Pose, Gaze}`, `Eyes::gaze` (drowsy blinks 2.4× and 3× slower; paused never blinks) | `eyes::a_paused_agent_rests_its_eyes`, `avatar::tests` |
| reduced motion: still poses | | `Eyes::gaze`, `Eyes::next_change` (no frames) | `eyes::reduced_motion_keeps_the_pose_still`, `a_living_avatar_asks_for_frames_only_while_its_eyes_move` |

The glance moves the eyes in one frame and holds them. Its ease on the snappy spring: 0.6.1: deferred by owner decision (2026-11-22).

## 02 Approve

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| the button's fill compresses | `distance.pressFill` 0.94 | `ui::primitives::button` press fill | see 10 |
| stamp 3 px down | `spring.playful` | `ui::chat::approval::Approving` (a kick on the playful spring) | `approval::the_stamp_dips_about_three_px_flashes_and_closes_after_the_beat` |
| the edge flashes ochre and settles to ink | | `Approving::flash` | same test. The ochre settles within the 260 ms beat, when the card closes (the design's 560 ms runs past the close) |
| collapse into the record after a 260 ms beat | `spring.calm` | `Approving::step` (`BEAT`) | `approval::the_record_arrives_on_the_calm_spring_after_the_beat`, `interrupt_undo_halfway` |
| the record's check draws on | `spring.calm` | `motion::check::DrawOn` | `check::draws_on_and_snaps`, `the_last_of_the_stroke_draws_on_too` |
| Deny lets go quietly; Ctrl+Enter snaps | | `approval::respond` | `approval::a_key_snaps_with_no_frames_in_between`, `views::keyboard_snaps` |

The followers (the header's state, the Recents mark, the composer hint) change when the daemon's state does, not on a
stagger.

## 03 Message arrival

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| tool rows tick in | `spring.snappy`, `distance.rise` 8 | `ui::chat::rows::step_motion` (a step drawn while it runs rises into place as it fades in) | `chat::tests::tool_rows::a_running_step_ticks_in_and_its_check_draws_on`, `an_older_turns_steps_show_still`, `under_reduced_motion_a_step_shows_at_once` |
| their checks draw on | `spring.calm` | `motion::check::DrawOn` in `step_motion` | same tests |
| words fade in 34 ms apart, per run | `duration.base` 280, 34 ms | gpui-kit's `TextViewMotion` stream fade (`ui::files::markdown::styled`); gpui-kit follows `reduce_motion` | gpui-kit's own tests; scenario `motion-arrival` |
| the code block grows to its height, lines 40 ms apart | `spring.calm`, `stagger.tight` | n/a on desktop: the code block is drawn inside gpui-kit's `TextView`, which has no height motion, and the pins forbid patching gpui-kit before the release (plan "Pins and upgrades"); its lines appear as they stream, fading word by word | n/a |
| the bottom-anchored thread glides as it grows | `duration.base`, `easing.out` | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile the thread (`ui::chat::thread`) follows its newest message with GPUI's `FollowMode::Tail`, which jumps | none in 0.6.0 |

## 04 Composer

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| the card lifts 2 px on focus | `distance.lift` 2, `spring.calm` | `ui::chat::composer::Composer` (`lift`); focus by Tab lands at once | composer view tests |
| it grows to 76 px | | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile the card keeps its height on focus | none in 0.6.0 |
| the model pill grows into its menu | `spring.snappy`, 90 ms | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile the menu opens at once under the pill (gpui-kit's menu) | none in 0.6.0 |
| the dictation meter, a spring per bar | `spring.snappy` | 0.6.1: deferred by owner decision (2026-11-22). The microphone capture (`desktop-host::speech`) reports no level to draw yet; 0.5.x had no meter | none in 0.6.0 |
| the send button pops awake | `spring.snappy` | `motion::keyed::spring` on `IconButton::icon_scale` (the composer's round button) | the keyed spring's tests (10) |
| the draft flies into the thread (FLIP) | `spring.calm` | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile the new message appears in the thread at once | none in 0.6.0 |

## 05 Phone drawer

n/a on desktop: the sidebar is persistent (plan T7.1 item 6); a narrow window gets the compact navigation instead (see
the end).

## 06 Numbers roll

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| the Needs you count rolls, digit by digit | `spring.playful` | `motion::digits::Rolling` (Agents overview) | `digits::a_changed_digit_rolls_and_the_rest_stays`, `a_roll_lands_on_its_digit_without_a_jump`, `a_length_change_snaps_the_whole_number`; scenario `motion-numbers` |
| spend and quota roll; bars ease to their length | `spring.playful`, `spring.calm` | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile Usage and Accounts show the new figures and bar lengths at once | none in 0.6.0 |

## 07 The stone mark

n/a on desktop: no desktop screen draws the mark. The design puts it only on the shell's "nothing open yet" panel, and the
desktop app opens on Agents (as 0.5.x opened the first chat), so that panel never shows.

## 08 First open

n/a on desktop: phone only (plan T7.1 item 9).

## 09 Toast and sheet

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| a toast drops in from the top with a settle | `spring.playful`, 16 px | `ui::primitives::toast` (`ToastMotion`) | `toast::a_toast_drops_in_and_rises_out_faster`; scenario `motion-toast` |
| and leaves the way it came, faster | `duration.quick` 200, 8 px, `easing.out` | same; closed by a key it is gone at once | same; `toast::under_reduced_motion_a_toast_shows_at_once`; scenario `motion-keyboard` |
| the sheet | `spring.calm` | `ui::primitives::sheet` (gpui-kit's sheet, which follows `reduce_motion`) | `reduced::gpui_animations_and_the_sheet_follow_a_live_change`. The drag and the flick are phone only |

## 10 Press, hover and lift

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| a pressed fill compresses, its words stay; letting go springs it back | `distance.pressFill` 0.94, `spring.snappy` | `ui::primitives::button::{press_motion, press_fill}` on `motion::keyed::spring`: the pointer's press springs the fill in and back with the snappy settle; a key's press shows none | `button::a_press_springs_the_fill_in_and_back`. The 140 ms ease in: 0.6.1: deferred by owner decision (2026-11-22); meanwhile the fill springs in on the snappy spring too |
| an icon button may scale whole | `spring.snappy` | `IconButton::icon_scale` (the send button's pop) | the keyed spring's tests |
| hover | `duration.hover` 200 | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile GPUI's hover style changes at once | none in 0.6.0 |
| glass cards lift 2 px as their shadow deepens | `distance.lift`, `spring.calm` | 0.6.1: deferred by owner decision (2026-11-22). Meanwhile cards keep their place and shadow on hover | none in 0.6.0 |
| toggles land with the snappy settle | `spring.snappy` | `ui::primitives::toggle::Toggle` on `motion::keyed::spring` | `toggle::the_knob_springs_across_after_a_click_and_lands_at_once_after_a_key` |

## 11 Lists reorder and reflow

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| a Recents row slides to its new place, the others make room (FLIP) | `spring.calm` | `motion::flip::Flip` in `ui::shell::sidebar::RecentsMotion` | `flip::*`, `shell::recents_slide_to_new_places_and_snap_under_reduced_motion`; scenario `motion-recents` |
| a row that leaves fades and slides 12 px out while the gap closes | `spring.calm` | `RecentsMotion` (`leaving_row`, from where it was painted) | `sidebar::a_row_that_leaves_fades_out_from_where_it_was`, `under_reduced_motion_a_row_leaves_at_once_and_a_returning_row_stops_leaving` |

The design moves a reorder by travel time on the move curve (74 px: 313 ms); the desktop keeps T2.3's calm spring, which
turns mid-flight without a jump, so a reorder settles in about 560 ms.

## 12 Browser panel

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| the panel opens and closes while the chat resizes; its content is revealed, not squashed | `spring.calm` | `ui::browser::motion` (T3.7), the live variant by default with the snapshot fallback (`panel_variant::choose`) | `panel::keyboard_toggle_snaps`, `motion::tests::interrupt_continues_from_value`, `panel_variant::tests::choose`, `e2e_panel_open_close`; scenario `motion-panel` |
| the live dot pulses while frames arrive | `duration.pulse` 1600, `easing.pulse` | 0.6.1: deferred by owner decision (2026-11-22). The panel's control bar has no "Live, 58 fps" mark yet (T3.7 shipped without it) | none in 0.6.0 |

## Shared elements (README "Shared elements across screens")

| Motion | Tokens | Implemented by | Check |
|---|---|---|---|
| the avatar flies from its Recents row, Agents card or Needs you card into the chat header; both ends hide their copy | `spring.calm` | `motion::flight::{Flight, SharedAvatar, take_off, frame}` | `flight::flies_from_the_row_to_the_header`, `a_recall_mid_flight_keeps_its_velocity`, `chat::tests::shared_avatar::the_avatar_flies_from_its_row_into_the_chat_header`, `the_avatar_flies_from_its_card_into_the_chat_header`, `in_a_narrow_window_the_avatar_flies_from_its_card` (the flight keeps the bounds it took off from: a card's screen is gone from the chat's first frame); scenario `motion-flight` |
| opened by a key, or under reduced motion: no flight | | `take_off` | `chat::tests::shared_avatar::a_key_opens_the_chat_with_no_flight`; scenarios `motion-keyboard`, `motion-reduced` |

The phone's Needs you item becoming the approval card is phone only.

## The narrow window (not a motion.html card)

Below 640 design px wide (`ui::shell::narrow`), the sidebar becomes the compact navigation (`sidebar::compact`: the five
sections as icon links, then New agent) and the side panels open as sheets over the content: the browser panel, an
exchange and a room's members (`shell::sheet`, `members::panel`). Checks: `projects::tests::a_narrow_window_has_compact_navigation_and_projects_fits`,
`rooms::tests::a_narrow_window_shows_the_members_as_a_sheet`.

## Deferred to 0.6.1 (owner decision, 2026-11-22)

0.6.0 ships the motions above that have a check; these wait for 0.6.1:

- 01: the glance's ease on the snappy spring (it moves in one frame).
- 03: the bottom-anchored glide.
- 04: the composer growing to 76 px, the pill growing into its menu, the dictation meter's springs, the send flight.
- 06: spend and quota rolling, bars easing.
- 10: the press's 140 ms ease in (the fill springs in and back on the snappy spring meanwhile), the hover fade, the
  cards' lift.
- 12: the live mark and its pulsing dot.

Not deferred, n/a on desktop: 03's code block growing (drawn inside gpui-kit), 05 the phone drawer, 07 the stone mark,
08 first open.
