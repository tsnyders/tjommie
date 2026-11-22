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
pub mod keyed;
pub mod reduced;
pub mod spring;

use gpui::App;

/// Starts the runtime: the clock and reduced motion. In a test-driver build the motion checks' switches come from the
/// command line: `--trace <csv>` writes the frame log, `--force-reduced` reduces motion. A production build has neither
/// (M7 review m6): it writes no trace and follows Windows' setting.
pub fn init(cx: &mut App) {
    #[cfg(feature = "test-driver")]
    let (trace, force_reduced) = {
        // args_os: a non-Unicode argument must not panic the start (M2 review F12).
        let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
        let trace = args
            .iter()
            .position(|arg| arg == "--trace")
            .and_then(|at| args.get(at + 1))
            .map(std::path::PathBuf::from);
        (trace, args.iter().any(|arg| arg == "--force-reduced"))
    };
    #[cfg(not(feature = "test-driver"))]
    let (trace, force_reduced) = (None, false);
    clock::init(trace, cx);
    reduced::init(force_reduced, cx);
}
