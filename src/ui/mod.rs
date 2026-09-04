//! egui view code. Each submodule renders one region of the window and takes
//! whatever slice of app state it needs, so the panels stay independently
//! readable.
//!
//! Panels never mutate app state directly. They return an `Action` enum
//! describing what the user asked for, which `app.rs` applies. That keeps the
//! borrow checker happy and makes the state transitions easy to follow in one
//! place.

pub mod file_list;
pub mod missing_vips;
pub mod preview_panel;
pub mod settings_panel;
pub mod toolbar;
