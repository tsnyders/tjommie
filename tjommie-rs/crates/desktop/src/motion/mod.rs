//! The motion runtime (Phase 3 plan T2.3): the spring that keeps its velocity (GPUI limit 13), the window's clock and
//! frame log with the inactive-window throttle (gate condition 5), reduced motion that follows Windows (limit 18),
//! keyboard snapping, and what the motions are made of: FLIP (limit 14), the shared-element flight, rolling digits,
//! the draw-on check and the eyes (limit 15). M7 builds every motion.html motion from these.

pub mod check;
pub mod clock;
pub mod digits;
pub mod eyes;
pub mod flight;
pub mod flip;
pub mod keyboard;
pub mod reduced;
pub mod spring;

use std::path::PathBuf;

use gpui::App;

/// Starts the runtime from the command line: the clock (`--trace <csv>` writes its frame log) and reduced motion
/// (`--force-reduced`, for tests).
pub fn init(cx: &mut App) {
    let args: Vec<String> = std::env::args().collect();
    let trace = args
        .iter()
        .position(|arg| arg == "--trace")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from);
    clock::init(trace, cx);
    reduced::init(args.iter().any(|arg| arg == "--force-reduced"), cx);
}
