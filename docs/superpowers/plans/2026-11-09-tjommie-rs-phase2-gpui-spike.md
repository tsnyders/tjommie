# Phase 2: GPUI spike and owner gate

- **Status:** draft for owner approval, 2026-11-09. Time-boxed to 10 working days on the owner's PC. It runs alongside Phase 1.
- **Spec:** §5 Phase 2 (criteria 1–8 and the gate table), §3.9 (stream target), §1 (Aegis).
- **Tag:** UI work. Throwaway code under `tjommie-rs/spikes/gpui/`. Nothing here ships.

## Setup and safety
- **Crate:** `tjommie-rs/spikes/gpui/` is its own binary, `tjommie-spike.exe`. `gpui` and `gpui-component` are pinned with `git = …, rev = "<sha>"`. Record both SHAs in the report.
- **Data and identity:**
  - All data, including WebView2 user-data folders, goes under `%LOCALAPPDATA%\Tjommie-spike`.
  - No installer, no ports, no Credential Manager.
  - It never reads or writes `%LOCALAPPDATA%\Tjommie`.
- **Spawning:** every spawn goes through the Phase 1 `spawn::command()`, or the same flag if Phase 1 isn't merged yet.
- **Running:** `cargo run --release` is allowed for the spike. It is not release packaging.
- **Inputs:** the Aegis tokens (`design/claude-design/aegis/tokens.json`), the fonts listed in `aegis/README.md` (Fonts) and the rendering table.

## Measurement method (same for every frame-time number)
- **Tool:** PresentMon (open source; per-process present intervals), filtered to `tjommie-spike.exe`. Use `MsBetweenPresents`.
- **Report** the display refresh rate. Set Windows scaling to 100% unless the criterion says otherwise.
- **Runs:** each scenario is 3 runs. Report p50, p95 and max. Pass is p95 ≤ 16.7 ms on all 3 runs.
- **Scripting:** a `--scenario <name>` flag drives scenarios, so they repeat without a human. Motion scenarios loop the motion 50 times per run.
- **Interruption check:** reverse the motion at 50% of its duration. The animated value is logged every frame (`--trace` writes CSV). Pass needs two things:
  - No frame-to-frame step is larger than the largest step of the uninterrupted run.
  - The motion ends at the correct final layout.
- **Reduced motion:** turn off Windows Settings > Accessibility > Visual effects > Animation effects (`SPI_GETCLIENTAREAANIMATION`). Pass if every scenario's state change completes within one frame.
- **Keyboard check:** trigger the same state changes by keyboard. Pass if nothing animates (spec, Aegis README "Motion").

## Schedule
The browser comes first because it is the riskiest part, and it gives the Aegis motion spec time to be finished.

| Days | Work | Criteria |
|---|---|---|
| 1–2 | Window shell with Aegis tokens and fonts: sidebar, a chat column with 10,000 virtualized messages, composer card | 1 |
| 3–7 | Per-bot browser. Try engines in the order (a), (b), (c) from spec §5 Phase 2, and stop at the first that passes all of 2–6. For each engine, record what failed. Measure the §3.9 stream target from the chosen engine with the same fixture page and method as Phase 1 T8.5 | 2–7 |
| 8–9 | The three Aegis motions, with timings, easings, springs and staggers taken from `aegis/components/motion.html` and the `motion` tokens. If `motion.html` is not final by day 8, stop and tell the owner; don't guess | 8 |
| 10 | Baseline (below) and the report | — |

The three motions:
1. **Browser panel opens while the chat column resizes:** panel slide plus column width, together.
2. **FLIP reorder of the sidebar recents:** a bot moves to the top when it gets activity. Record old positions, re-lay out, then animate offsets to zero.
3. **Approval card resolves into a record while the content below slides up:** collapse plus follow-up.

**Baseline:** measured on the installed production Electron app (0.4.2 or R1; record which), read-only.
- Cold start: from process start to the first present, measured with PresentMon.
- Idle working set: the sum of `WorkingSet64` over all Tjommie processes after 60 s idle.
- Installer size in bytes.

Measure the spike the same way, where it applies.

## Report template
Save the report as `tjommie-rs/spikes/gpui/REPORT.md`, then hand it to the owner.

```
# GPUI spike report: <date>
gpui rev: <sha>   gpui-component rev: <sha>   display: <Hz>, scaling <..>
## Results
| # | Criterion | Pass/Fail | Numbers (p50/p95/max or notes) | Evidence (CSV/screenshot path) |
| 1 | 10k list scroll | | | |
| 2 | Per-bot browser, engine chosen: a/b/c | | why the earlier engines failed | |
| 3 | Overlays over the browser | | | |
| 4 | Input forwarding, take control/return | | | |
| 5 | Alignment at 100/125/150% and app zoom | | | |
| 6 | Accessibility snapshot, stream (median fps, p95 gap), private mode | | | |
| 7 | No console window | | | |
| 8a | Motion: browser panel and column | | p95, interruption, reduced motion, keyboard | |
| 8b | Motion: FLIP recents | | | |
| 8c | Motion: approval resolve | | | |
## Baseline vs spike
| Metric | Electron (<version>) | Spike |
## Risks found
<pre-1.0 breakages, workarounds needed, unknowns>
## Options for the owner (from spec §5 Phase 2)
GPUI / Electron restyled to Aegis / Tauri 2: costs and gains, updated with what the spike found.
Planner's reading of the results: <one paragraph, not a decision>
## Owner decision
<filled in by the owner; then recorded in spec §5 Phase 2>
```

**Done when:**
- The report is complete.
- The owner has recorded the gate decision in spec §5 Phase 2.
- The spike crate stays unshipped. Delete it or leave it; the owner decides.
