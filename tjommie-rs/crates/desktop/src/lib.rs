//! Tjommie's desktop app (Phase 3 plan): the GPUI shell in the Aegis design.

pub mod app;
pub mod assets;
pub mod guard;
pub mod keymap;
pub mod model;
pub mod motion;
pub mod notices;
pub mod semantic;
#[cfg(feature = "test-driver")]
pub mod test_driver;
pub mod theme;
pub mod ui;
pub mod zoom;
