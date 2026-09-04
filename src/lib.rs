//! vips-gui internals, exposed as a library so integration tests can drive
//! the conversion pipeline without going through the GUI.
//!
//! The binary in `main.rs` is a thin shell around [`app::App`].

pub mod app;
pub mod diagnostics;
pub mod job;
pub mod settings;
pub mod ui;
pub mod vips;
pub mod worker;
