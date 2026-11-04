//! Minimized window tiles and thumbnail handling.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::config::Entry;
use crate::kwin_windows::{self, AppWindow, KWinWindows, WindowSnapshot};
use crate::thumbnail::Thumbnailer;

use super::animations::EXPAND_MILLIS;
use super::icon::{icon_for, picture_for, RoundedIcon};
use super::launcher::settings_window_action;
use super::tiles::{create_tile, DockTile, HoverLabel, PinState};
use super::windows::run_active_window_action;

pub(crate) struct MinimizedTile {
    pub(crate) window_id: String,
    pub(crate) revealer: gtk::Revealer,
    pub(crate) button: gtk::Button,
    pub(crate) badge: Option<RoundedIcon>,
    /// Shows the real window thumbnail once one has been captured.
    pub(crate) picture: gtk::Picture,
    /// The placeholder title bar, shown until there is a thumbnail.
    pub(crate) placeholder: gtk::Box,
    /// Width / height of the thumbnail (or of the placeholder card).
    pub(crate) aspect: std::cell::Cell<f64>,
}

pub(crate) const PLACEHOLDER_ASPECT: f64 = 1.625;

/// Tiles are as tall as the Dock allows and as wide as their window's shape,
/// within limits so a very wide or very tall window still fits the Dock.
pub(crate) fn minimized_tile_size(icon_size: i32, aspect: f64) -> (i32, i32) {
    let height = icon_size * 4 / 5;
    let width = (f64::from(height) * aspect).round() as i32;
    (width.clamp(height * 9 / 10, height * 2), height)
}

/// Shows `texture` on the tile. Returns true if the tile's shape changed, so
/// the caller must size it again.
pub(crate) fn set_minimized_thumbnail(tile: &MinimizedTile, texture: &gdk::Texture) -> bool {
    tile.picture.set_paintable(Some(texture));
    tile.picture.set_visible(true);
    tile.placeholder.set_visible(false);
    let aspect = f64::from(texture.width()) / f64::from(texture.height().max(1));
    (tile.aspect.replace(aspect) - aspect).abs() > 0.02
}

pub(crate) fn size_minimized_tile(tile: &MinimizedTile, icon_size: i32) {
    let (width, height) = minimized_tile_size(icon_size, tile.aspect.get());
    tile.button.set_size_request(width, height);
    if let Some(badge) = &tile.badge {
        badge.set_icon_size((height * 55 / 100).max(16));
    }
}

/// The Dock entry whose icon and name stand for `window`'s application: the
/// matching Dock tile if there is one, else one derived from its identity.
pub(crate) fn entry_for_window(window: &kwin_windows::AppWindow, app_tiles: &[DockTile]) -> Entry {
    if let Some(tile) = app_tiles.iter().find(|tile| window.matches(&tile.app_aliases)) {
        return tile.entry.clone();
    }
    let identity = window.resolved_identity();
    let name = if identity == kwin_windows::SETTINGS_IDENTITY {
        crate::settings::WINDOW_TITLE.to_string()
    } else {
        kwin_windows::installed_app_for_identity(&identity)
            .map(|app| app.name().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| identity.rsplit('.').next().unwrap_or(&identity).to_string())
    };
    Entry {
        name,
        icon: None,
        command: identity,
    }
}

pub(crate) fn create_minimized_tile(
    window: &kwin_windows::AppWindow,
    entry: Entry,
    display: &gdk::Display,
    hover: &HoverLabel,
) -> MinimizedTile {
    let title = match window.title.trim() {
        "" => entry.name.clone(),
        title => title.to_string(),
    };
    let button = gtk::Button::builder()
        .css_classes(["dock-item", "dock-minimized-window"])
        .focusable(false)
        .valign(gtk::Align::Center)
        .build();
    hover.attach(&button, &title);

    // The window "thumbnail": a small window with a title bar.
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("dock-minimized-thumb");
    card.set_hexpand(true);
    card.set_vexpand(true);
    // Rounds the corners of the thumbnail too.
    card.set_overflow(gtk::Overflow::Hidden);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    bar.add_css_class("dock-minimized-titlebar");
    for _ in 0..3 {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dock-minimized-dot");
        dot.set_size_request(4, 4);
        dot.set_valign(gtk::Align::Center);
        bar.append(&dot);
    }
    card.append(&bar);
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Fill);
    picture.set_can_shrink(true);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_visible(false);
    // Rounded like the card, and inside its 1px border.
    picture.add_css_class("dock-minimized-picture");
    picture.set_overflow(gtk::Overflow::Hidden);
    picture.set_margin_start(3);
    picture.set_margin_end(3);
    picture.set_margin_top(3);
    picture.set_margin_bottom(3);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&card));
    // A picture's natural size is the screenshot's pixel size. As an overlay
    // that is not measured, the tile keeps the size the Dock gives it.
    overlay.add_overlay(&picture);
    overlay.set_measure_overlay(&picture, false);
    let badge = icon_for(&entry, display).map(|icon| RoundedIcon::new(&icon.picture));
    match &badge {
        Some(badge) => {
            badge.set_halign(gtk::Align::End);
            badge.set_valign(gtk::Align::End);
            badge.set_can_target(false);
            overlay.add_overlay(badge);
        }
        None => {
            let letter = entry
                .name
                .chars()
                .next()
                .map_or("?".to_string(), |c| c.to_uppercase().to_string());
            let label = gtk::Label::new(Some(&letter));
            label.add_css_class("dock-minimized-letter");
            label.set_halign(gtk::Align::End);
            label.set_valign(gtk::Align::End);
            label.set_can_target(false);
            overlay.add_overlay(&label);
        }
    }
    button.set_child(Some(&overlay));

    let window_id = window.window_id.clone();
    let identity = window.resolved_identity();
    button.connect_clicked(move |_| {
        if identity == kwin_windows::SETTINGS_IDENTITY {
            settings_window_action("activate");
        } else {
            run_active_window_action(Some(window_id.clone()), "restore");
        }
    });

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
    revealer.set_transition_duration(0);
    revealer.set_child(Some(&button));
    revealer.set_reveal_child(false);
    revealer.set_visible(false);
    revealer.set_transition_duration(EXPAND_MILLIS as u32);

    MinimizedTile {
        window_id: window.window_id.clone(),
        revealer,
        button,
        badge,
        picture,
        placeholder: bar,
        aspect: std::cell::Cell::new(PLACEHOLDER_ASPECT),
    }
}

/// Makes the minimised-window tiles match `snapshot`: one per minimised
/// window while `show` is true (the "minimise into icon" setting is off), none
/// otherwise. Returns whether tiles were added or removed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_minimized_tiles(
    snapshot: &WindowSnapshot,
    show: bool,
    app_tiles: &[DockTile],
    tiles: &mut Vec<MinimizedTile>,
    row: &gtk::Box,
    display: &gdk::Display,
    hover: &HoverLabel,
    icon_size: i32,
    thumbnails: &Thumbnailer,
    animate_layout: &dyn Fn(u64),
    settle: &Rc<dyn Fn()>,
) -> bool {
    let desired: Vec<&kwin_windows::AppWindow> = if show {
        snapshot
            .windows
            .iter()
            .filter(|window| {
                window.minimized && !window.window_id.is_empty() && window.is_trackable_app()
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut changed = false;

    for index in (0..tiles.len()).rev() {
        if desired
            .iter()
            .any(|window| window.window_id == tiles[index].window_id)
        {
            continue;
        }
        let tile = tiles.remove(index);
        animate_layout(EXPAND_MILLIS + 80);
        tile.revealer.set_reveal_child(false);
        let row = row.clone();
        let settle = Rc::clone(settle);
        glib::timeout_add_local_once(Duration::from_millis(EXPAND_MILLIS + 20), move || {
            if tile.revealer.parent().is_some() {
                row.remove(&tile.revealer);
            }
            row.set_visible(row.first_child().is_some());
            settle();
        });
        changed = true;
    }

    for window in desired {
        if tiles.iter().any(|tile| tile.window_id == window.window_id) {
            continue;
        }
        let entry = entry_for_window(window, app_tiles);
        let tile = create_minimized_tile(window, entry, display, hover);
        match thumbnails.texture(&window.window_id) {
            Some(texture) => {
                set_minimized_thumbnail(&tile, &texture);
            }
            // Never seen while visible (e.g. minimised before the Dock
            // started): worth one attempt, though KWin may refuse.
            None => thumbnails.request(&window.window_id),
        }
        size_minimized_tile(&tile, icon_size);
        row.append(&tile.revealer);
        row.set_visible(true);
        animate_layout(EXPAND_MILLIS + 80);
        tile.revealer.set_visible(true);
        tile.revealer.set_reveal_child(true);
        tiles.push(tile);
        changed = true;
    }
    changed
}

pub(crate) fn transient_running_entries(
    snapshot: &WindowSnapshot,
    configured_tiles: &[DockTile],
    pin_state: &PinState,
) -> Vec<(String, Entry)> {
    let mut groups = std::collections::BTreeMap::<String, Vec<&kwin_windows::AppWindow>>::new();
    for window in snapshot
        .windows
        .iter()
        .filter(|window| window.is_trackable_app())
    {
        if configured_tiles
            .iter()
            .filter(|tile| {
                tile.dynamic_identity.is_none() || pin_state.is_pinned(&tile.command)
            })
            .any(|tile| window.matches(&tile.app_aliases))
        {
            continue;
        }
        let identity = window.resolved_identity();
        if identity.starts_with("pid:") {
            continue;
        }
        groups.entry(identity).or_default().push(window);
    }

    groups
        .into_iter()
        .map(|(identity, _windows)| {
            let app = kwin_windows::installed_app_for_identity(&identity);
            let name = if identity == kwin_windows::SETTINGS_IDENTITY {
                crate::settings::WINDOW_TITLE.to_string()
            } else {
                app.as_ref()
                    .map(|app| app.name().to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| {
                        identity.rsplit('.').next().unwrap_or(&identity).to_string()
                    })
            };
            (
                identity.clone(),
                Entry {
                    name,
                    icon: None,
                    command: identity,
                },
            )
        })
        .collect()
}

pub(crate) fn sync_transient_tiles(
    snapshot: &WindowSnapshot,
    tiles: &mut Vec<DockTile>,
    row: &gtk::Box,
    display: &gdk::Display,
    tracker: Option<KWinWindows>,
    launching: Rc<RefCell<HashMap<String, Instant>>>,
    pin_state: &PinState,
    hover: &HoverLabel,
) -> bool {
    let desired = transient_running_entries(snapshot, tiles, pin_state);
    let desired_ids: HashSet<_> = desired
        .iter()
        .map(|(identity, _)| identity.as_str())
        .collect();
    let mut changed = false;

    for index in (0..tiles.len()).rev() {
        if tiles[index]
            .dynamic_identity
            .as_deref()
            .is_some_and(|identity| !desired_ids.contains(identity))
            && !pin_state.is_pinned(&tiles[index].command)
        {
            row.remove(&tiles[index].revealer);
            tiles.remove(index);
            changed = true;
        }
    }

    for (identity, entry) in desired {
        if tiles
            .iter()
            .any(|tile| tile.dynamic_identity.as_deref() == Some(identity.as_str()))
        {
            continue;
        }
        let tile = create_tile(
            entry,
            display,
            tracker.clone(),
            Rc::clone(&launching),
            vec![identity.clone()],
            false,
            false,
            true,
            Some(identity),
            pin_state.clone(),
            hover,
        );
        // Start collapsed: the refresh loop slides it in and bounces it.
        tile.revealer.set_reveal_child(false);
        tile.revealer.set_visible(false);
        row.append(&tile.revealer);
        tiles.push(tile);
        changed = true;
    }
    changed
}
