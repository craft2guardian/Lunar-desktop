//! GTK's own animations (popover fade-in/out, switch slides, ...) are off by
//! default so menus appear and disappear instantly. The Dock's hand-made
//! animations (icons fading away, the dock shrinking, auto-hide) switch them
//! on for just as long as they need.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;

thread_local! {
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// Turns GTK animations off. Call once at startup.
pub fn disable_by_default() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
}

/// Turns GTK animations on for `milliseconds`, then back off.
pub fn allow_for(milliseconds: u64) {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    settings.set_gtk_enable_animations(true);
    let token = GENERATION.with(|generation| {
        let next = generation.get().wrapping_add(1);
        generation.set(next);
        next
    });
    glib::timeout_add_local_once(std::time::Duration::from_millis(milliseconds), move || {
        if GENERATION.with(Cell::get) == token {
            settings.set_gtk_enable_animations(false);
        }
    });
}
