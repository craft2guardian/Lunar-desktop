//! The Dock: a small centred `top`-layer surface at the bottom of each output.
//! Its tiles come from `config/dock.toml` (see `config.rs`). Clicking a tile
//! launches its command. Running apps get a small indicator beneath the icon.

mod animations;
mod apps;
mod icon;
mod launcher;
mod layout;
mod previews;
mod tiles;
mod windows;

// Public API used by desktop.rs and menubar.rs
pub use launcher::{desktop_file_for, spawn_detached};
pub(crate) use launcher::launch_app_detached;
pub use layout::window;
pub use windows::{run_active_window_action, run_window_action};
