//! Bounce and expand animation helpers for Dock tiles.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::glib;

use super::tiles::DockTile;

/// How long a tile takes to slide into the Dock (matches the Revealer's
/// transition duration set in `create_tile`). The bounce starts right after.
pub(crate) const EXPAND_MILLIS: u64 = 180;

pub(crate) fn expand_tile(tile: &DockTile, animate_layout: &dyn Fn(u64), bounce: Option<Duration>) {
    // Animations must be on *before* the revealer starts.
    animate_layout(EXPAND_MILLIS + 80);
    tile.revealer.set_visible(true);
    tile.revealer.set_reveal_child(true);
    let Some(bounce) = bounce else {
        return;
    };
    let bounce_for = Rc::clone(&tile.bounce_for);
    let revealer = tile.revealer.downgrade();
    glib::timeout_add_local_once(Duration::from_millis(EXPAND_MILLIS + 20), move || {
        if revealer.upgrade().is_some_and(|revealer| revealer.reveals_child()) {
            bounce_for(bounce);
        }
    });
}
