//! Tjommie's desktop app (Phase 3 plan): the GPUI shell in the Aegis design.

pub mod app;
pub mod app_controls;
pub mod assets;
pub mod guard;
pub mod hosts;
pub mod keymap;
pub mod live;
pub mod model;
pub mod motion;
pub mod notices;
pub mod semantic;
pub mod services;
pub mod speech;
#[cfg(any(test, feature = "test-driver"))]
pub mod test_driver;
pub mod theme;
pub mod ui;
pub mod zoom;
