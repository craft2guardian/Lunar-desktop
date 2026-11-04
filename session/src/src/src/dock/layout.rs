//! Main Dock window construction, positioning, and refresh loops.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, Entry};
use crate::kde_shell::{self, Role};
use crate::kwin_windows::{self, KWinWindows, WindowSnapshot};
use crate::thumbnail::Thumbnailer;

use super::animations::{expand_tile, EXPAND_MILLIS};
use super::apps::{app_is_running, detect_desktop, running_apps, running_process_names, DesktopKind};
use super::icon::{icon_for, RoundedIcon};
use super::launcher::{launch_entry, program_name};
use super::previews::*;
use super::tiles::*;
use super::windows::*;

/// Icon tile size in logical px, scaled with the output height (clamped).
pub(crate) fn tile_size(monitor: &gdk::Monitor, preference: f64) -> i32 {
    let base = (monitor.geometry().height() as f64 * 0.058).clamp(46.0, 80.0);
    (base * (0.65 + preference / 100.0 * 0.7)).round() as i32
}


pub(crate) fn position_sync(
    window: &gtk::Window,
    monitor: &gdk::Monitor,
    update_position: kde_shell::PositionUpdater,
) -> Rc<dyn Fn()> {
    let weak_window = window.downgrade();
    let monitor = monitor.clone();
    Rc::new(move || {
        let Some(window) = weak_window.upgrade() else {
            return;
        };
        let (width, height) = measured_window_size(&window, &monitor);
        let geometry = monitor.geometry();
        let x = geometry.x() + (geometry.width() - width) / 2;
        let y = geometry.y() + geometry.height() - height;
        update_position(x, y);
    })
}

pub(crate) fn measured_window_size(window: &gtk::Window, monitor: &gdk::Monitor) -> (i32, i32) {
    let geometry = monitor.geometry();
    let (minimum_width, natural_width, _, _) = window.measure(gtk::Orientation::Horizontal, -1);
    let width = natural_width
        .max(minimum_width)
        .max(1)
        .min(geometry.width());
    let (minimum_height, natural_height, _, _) = window.measure(gtk::Orientation::Vertical, width);
    (width, natural_height.max(minimum_height).max(1))
}

pub(crate) fn resize_window_to_content(window: &gtk::Window, monitor: &gdk::Monitor) {
    let (width, height) = measured_window_size(window, monitor);
    window.set_default_size(width, height);
}

/// Creates (but does not present) the dock window for `monitor`.
pub fn window(
    app: &gtk::Application,
    monitor: &gdk::Monitor,
    kwin_windows: Option<KWinWindows>,
) -> gtk::Window {
    let display = monitor.display();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    let dock = gtk::CenterBox::new();
    dock.add_css_class("dock");
    let dock_content = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    dock_content.append(&row);
    let resize_handle = gtk::Overlay::new();
    resize_handle.set_size_request(12, 34);
    resize_handle.set_valign(gtk::Align::Center);
    resize_handle.set_halign(gtk::Align::Center);
    resize_handle.set_cursor_from_name(Some("ns-resize"));
    let separator = gtk::Box::new(gtk::Orientation::Vertical, 0);
    separator.set_size_request(1, 34);
    separator.set_valign(gtk::Align::Center);
    separator.set_halign(gtk::Align::Center);
    separator.add_css_class("dock-separator");
    resize_handle.set_child(Some(&separator));
    dock_content.append(&resize_handle);
    // Minimised windows (when they are not folded into their app icon) sit
    // between the separator and the Trash, as on macOS.
    let minimized_row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    minimized_row.set_valign(gtk::Align::Center);
    minimized_row.set_visible(false);
    dock_content.append(&minimized_row);
    let resize_origin = Rc::new(std::cell::Cell::new(50.0));
    let resize_gesture = gtk::GestureDrag::new();
    resize_gesture.set_button(1);
    {
        let resize_origin = Rc::clone(&resize_origin);
        resize_gesture.connect_drag_begin(move |_, _, _| {
            resize_origin.set(crate::preferences::current().dock_size);
        });
    }
    {
        let resize_origin = Rc::clone(&resize_origin);
        resize_gesture.connect_drag_update(move |_, _, dy| {
            crate::preferences::set_dock_size_live(resize_origin.get() - dy * 0.5);
        });
    }
    resize_gesture.connect_drag_end(|_, _, _| crate::preferences::persist_current());
    resize_handle.add_controller(resize_gesture);
    let (trash_button, trash_icon) = create_trash_item();
    dock_content.append(&trash_button);
    dock.set_center_widget(Some(&dock_content));

    let dock_revealer = gtk::Revealer::new();
    dock_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
    dock_revealer.set_transition_duration(220);
    dock_revealer.set_child(Some(&dock));
    dock_revealer.set_reveal_child(!crate::preferences::current().dock_auto_hide);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&dock_revealer);
    let hover = HoverLabel::new(&root);
    hover.attach(&trash_button, "Trash");
    // Keep a narrow hit target at the screen edge while the Dock is hidden.
    let reveal_sensor = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    reveal_sensor.set_size_request(260, 3);
    reveal_sensor.set_halign(gtk::Align::Center);
    root.append(&reveal_sensor);

    let mut entries = config::load_dock();
    let configured: HashSet<String> = entries.iter().map(|entry| entry.command.clone()).collect();
    for extra in config::load_pinned_extras() {
        if !configured.contains(&extra.command) {
            entries.push(extra);
        }
    }
    let pins = Rc::new(RefCell::new(config::load_pins(&entries)));
    let extras: Vec<Entry> = entries
        .iter()
        .filter(|entry| !configured.contains(&entry.command))
        .cloned()
        .collect();
    let pin_state = PinState {
        pins: Rc::clone(&pins),
        extras: Rc::new(RefCell::new(extras)),
        configured: Rc::new(configured),
        dirty: Rc::new(std::cell::Cell::new(false)),
    };
    let initial_running = if kwin_windows.is_some() {
        HashSet::new()
    } else {
        running_process_names()
    };
    let launching = Rc::new(RefCell::new(HashMap::<String, Instant>::new()));
    let mut tiles: Vec<DockTile> = Vec::new();
    for entry in entries {
        let always_open = entry.name.eq_ignore_ascii_case("finder");
        let program = program_name(&entry.command);
        let aliases = app_aliases(program.as_deref());
        let is_running = kwin_windows
            .as_ref()
            .map(|tracker| kwin_windows::matching_window_state(&aliases, &tracker.snapshot()).0)
            .unwrap_or_else(|| {
                program
                    .as_deref()
                    .is_some_and(|program| app_is_running(program, &initial_running))
            });
        let is_pinned = pins.borrow().contains(&entry.command);
        eprintln!(
            "[dock-track-debug][tile-init] entry={:?} command={:?} aliases={aliases:?} running={is_running} pinned={} finder_always_open={always_open} visible={}",
            entry.name,
            entry.command,
            is_pinned,
            is_pinned || is_running || always_open
        );
        let tile = create_tile(
            entry,
            &display,
            kwin_windows.clone(),
            Rc::clone(&launching),
            aliases,
            is_pinned,
            always_open,
            is_running,
            None,
            pin_state.clone(),
            &hover,
        );
        row.append(&tile.revealer);
        tiles.push(tile);
    }

    let tiles = Rc::new(RefCell::new(tiles));
    let minimized_tiles = Rc::new(RefCell::new(Vec::<MinimizedTile>::new()));
    let window = gtk::Window::builder()
        .application(app)
        .title("Sonoma Dock")
        .decorated(false)
        .css_classes(["sonoma-dock"])
        .child(&root)
        .build();
    crate::preferences::apply_dark_mode(&window);
    let dock_blur = crate::context_menu::attach_window_widget_blur(&window, &dock, 20);
    // The dock's size changes while icons fade/slide; keep KWin's blur region
    // following it for the length of the animation.
    let animate_blur: Rc<dyn Fn(u64)> = {
        let dock_blur = Rc::clone(&dock_blur);
        Rc::new(move |duration_ms| {
            crate::animation::allow_for(duration_ms + 300);
            let started = Instant::now();
            let dock_blur = Rc::clone(&dock_blur);
            glib::timeout_add_local(Duration::from_millis(16), move || {
                dock_blur();
                if started.elapsed() < Duration::from_millis(duration_ms) {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        })
    };

    // Size now, and again if the output's mode changes.
    let displayed_dock_size = Rc::new(std::cell::Cell::new(
        crate::preferences::current().dock_size,
    ));
    let apply = {
        let monitor = monitor.clone();
        let size_tiles = Rc::clone(&tiles);
        let size_minimized = Rc::clone(&minimized_tiles);
        let displayed_dock_size = Rc::clone(&displayed_dock_size);
        move || {
            let size = tile_size(&monitor, displayed_dock_size.get());
            for tile in size_tiles.borrow().iter() {
                tile.button.set_size_request(size, size);
                if let Some(icon) = &tile.icon {
                    icon.set_icon_size(size - 4);
                }
            }
            for tile in size_minimized.borrow().iter() {
                size_minimized_tile(tile, size);
            }
            trash_button.set_size_request(size, size);
            trash_icon.set_icon_size((size - 8) * 9 / 10);
        }
    };
    apply();

    let geometry = monitor.geometry();
    let (min_width, natural_width, _, _) = window.measure(gtk::Orientation::Horizontal, -1);
    let width = natural_width.max(min_width).min(geometry.width());
    let (min_height, natural_height, _, _) = window.measure(gtk::Orientation::Vertical, width);
    let height = natural_height.max(min_height);
    window.set_default_size(width, height);
    let x = geometry.x() + (geometry.width() - width) / 2;
    let y = geometry.y() + geometry.height() - height;
    let update_position = kde_shell::configure_window(&window, monitor, Role::Panel, x, y);
    let reposition = position_sync(&window, monitor, update_position);
    let size_observer: Rc<dyn Fn(f64)> = {
        let displayed_size = Rc::clone(&displayed_dock_size);
        let apply = apply.clone();
        let weak_window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        Rc::new(move |size| {
            displayed_size.set(size);
            apply();
            if let Some(window) = weak_window.upgrade() {
                resize_window_to_content(&window, &monitor);
            }
            reposition();
        })
    };
    crate::preferences::observe_dock_size(&size_observer);
    // Keep the observer alive for the lifetime of this dock window.
    let retain_size_observer = Rc::clone(&size_observer);
    window.connect_map(move |_| {
        let _ = &retain_size_observer;
    });

    // Like `animate_blur`, and also keeps the Dock window sized to its content
    // on every frame so a tile sliding in grows the dock smoothly.
    let animate_layout: Rc<dyn Fn(u64)> = {
        let animate_blur = Rc::clone(&animate_blur);
        let window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        Rc::new(move |duration_ms| {
            animate_blur(duration_ms);
            let started = Instant::now();
            let window = window.clone();
            let monitor = monitor.clone();
            let reposition = Rc::clone(&reposition);
            glib::timeout_add_local(Duration::from_millis(16), move || {
                if let Some(window) = window.upgrade() {
                    resize_window_to_content(&window, &monitor);
                    reposition();
                }
                if started.elapsed() < Duration::from_millis(duration_ms) {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        })
    };

    // Re-measure and re-place the window the moment the slide animation has
    // really finished. Fixed timers alone can fire mid-animation (slow frames),
    // leaving a hidden Dock with the wrong size and position, so the
    // bottom-edge strip is no longer where the pointer can reach it.
    {
        let window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        dock_revealer.connect_child_revealed_notify(move |_| {
            if let Some(window) = window.upgrade() {
                resize_window_to_content(&window, &monitor);
                reposition();
            }
        });
    }

    let hide_generation = Rc::new(std::cell::Cell::new(0_u64));
    let show_dock = {
        let animate_layout = Rc::clone(&animate_layout);
        let revealer = dock_revealer.clone();
        let generation = Rc::clone(&hide_generation);
        let window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        move || {
            generation.set(generation.get().wrapping_add(1));
            if !revealer.reveals_child() {
                crate::animation::allow_for(500);
                // Grow the window with the dock as it slides up.
                animate_layout(320);
                revealer.set_reveal_child(true);
                let window = window.clone();
                let monitor = monitor.clone();
                let reposition = Rc::clone(&reposition);
                glib::timeout_add_local(Duration::from_millis(240), move || {
                    if let Some(window) = window.upgrade() {
                        resize_window_to_content(&window, &monitor);
                        reposition();
                    }
                    glib::ControlFlow::Break
                });
            }
        }
    };
    let sensor_motion = gtk::EventControllerMotion::new();
    {
        let show_dock = show_dock.clone();
        sensor_motion.connect_enter(move |_, _, _| show_dock());
    }
    reveal_sensor.add_controller(sensor_motion);

    let dock_motion = gtk::EventControllerMotion::new();
    {
        let show_dock = show_dock.clone();
        dock_motion.connect_enter(move |_, _, _| show_dock());
    }
    {
        let generation = Rc::clone(&hide_generation);
        let revealer = dock_revealer.clone();
        let window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        let animate_layout = Rc::clone(&animate_layout);
        dock_motion.connect_leave(move |_| {
            if !crate::preferences::current().dock_auto_hide {
                return;
            }
            let token = generation.get().wrapping_add(1);
            generation.set(token);
            let generation = Rc::clone(&generation);
            let revealer = revealer.clone();
            let window = window.clone();
            let monitor = monitor.clone();
            let reposition = Rc::clone(&reposition);
            let animate_layout = Rc::clone(&animate_layout);
            glib::timeout_add_local(Duration::from_millis(500), move || {
                if generation.get() == token && crate::preferences::current().dock_auto_hide {
                    crate::animation::allow_for(500);
                    animate_layout(320);
                    revealer.set_reveal_child(false);
                    let window = window.clone();
                    let monitor = monitor.clone();
                    let reposition = Rc::clone(&reposition);
                    glib::timeout_add_local(Duration::from_millis(250), move || {
                        if let Some(window) = window.upgrade() {
                            resize_window_to_content(&window, &monitor);
                            reposition();
                        }
                        glib::ControlFlow::Break
                    });
                }
                glib::ControlFlow::Break
            });
        });
    }
    dock.add_controller(dock_motion);

    let apply_monitor_size = apply.clone();
    let monitor_reposition = Rc::clone(&reposition);
    monitor.connect_notify_local(Some("geometry"), move |_, _| {
        apply_monitor_size();
        monitor_reposition();
    });

    // Re-measure the Dock window and put it back in place once layout settles.
    let settle: Rc<dyn Fn()> = {
        let window = window.downgrade();
        let monitor = monitor.clone();
        let reposition = Rc::clone(&reposition);
        Rc::new(move || {
            if let Some(window) = window.upgrade() {
                resize_window_to_content(&window, &monitor);
            }
            reposition();
        })
    };
    let thumbnails = Rc::new(Thumbnailer::new());
    let mut last_thumbnail_capture = None::<Instant>;
    let mut logged_thumbnail_summary = None::<(usize, usize)>;
    if kwin_windows.is_none() {
        eprintln!("[thumbnail] no KWin window tracker, so no minimised-window tiles or thumbnails");
    }
    let refresh_minimized_tiles = Rc::clone(&minimized_tiles);
    let refresh_minimized_row = minimized_row.clone();
    let refresh_settle = Rc::clone(&settle);
    let mut applied_minimize_into_icon = crate::preferences::current().minimize_into_icon;

    let desktop = detect_desktop();
    let refresh_tracker = kwin_windows.clone();
    let refresh_interval = if refresh_tracker.is_some() {
        Duration::from_millis(120)
    } else {
        Duration::from_secs(1)
    };
    let refresh_pins = Rc::clone(&pins);
    let refresh_pin_state = pin_state.clone();
    let refresh_hover = hover.clone();
    let refresh_animate_blur = Rc::clone(&animate_blur);
    let refresh_animate_layout = Rc::clone(&animate_layout);
    // Apps that were already open when the shell started just appear; only
    // apps opened afterwards get the expand-and-bounce.
    let started_at = Instant::now();
    let refresh_launching = Rc::clone(&launching);
    let refresh_reposition = Rc::clone(&reposition);
    let refresh_row = row.clone();
    let refresh_display = display.clone();
    let refresh_window = window.downgrade();
    let refresh_monitor = monitor.clone();
    let last_logged_ui_state = Rc::new(RefCell::new(HashMap::<
        String,
        (bool, bool, bool, bool, bool),
    >::new()));
    let refresh_log_state = Rc::clone(&last_logged_ui_state);
    let mut refresh_count = 0_u64;
    let mut last_logged_revision = None::<u64>;
    let mut last_synced_revision = None::<u64>;
    let mut applied_auto_hide = crate::preferences::current().dock_auto_hide;
    let refresh_displayed_size = Rc::clone(&displayed_dock_size);
    let refresh_revealer = dock_revealer.clone();
    let refresh_show_dock = show_dock.clone();
    let refresh_hide_generation = Rc::clone(&hide_generation);
    let refresh_window_for_preferences = window.downgrade();
    let refresh_monitor_for_preferences = monitor.clone();
    let refresh_reposition_for_preferences = Rc::clone(&reposition);
    // Apps started some other way (a launcher, a terminal, a script) bounce
    // too: watch for a pinned app's process appearing before its window does.
    {
        let watch_tiles = Rc::clone(&tiles);
        let watch_layout = Rc::clone(&animate_layout);
        let watch_dirty = Rc::clone(&pin_state.dirty);
        let seen = RefCell::new(HashMap::<String, bool>::new());
        glib::timeout_add_local(Duration::from_millis(400), move || {
            let tiles = watch_tiles.borrow();
            if tiles.iter().all(|tile| tile.button.root().is_none()) {
                return glib::ControlFlow::Break;
            }
            let processes = running_process_names();
            let mut seen = seen.borrow_mut();
            for tile in tiles.iter() {
                let Some(program) = tile.program.as_deref() else {
                    continue;
                };
                let started = app_is_running(program, &processes);
                let before = seen.insert(tile.command.clone(), started);
                // New process, and no window for it yet (no running indicator).
                if started && before == Some(false) && tile.indicator.opacity() < 0.5 {
                    if tile.revealer.reveals_child() {
                        (tile.launch_bounce)();
                    } else {
                        // Not on the Dock yet: expand it, then bounce.
                        expand_tile(tile, &*watch_layout, Some(Duration::from_secs(6)));
                        // If no window ever shows up, have the refresh loop
                        // take the tile away again.
                        let dirty = Rc::clone(&watch_dirty);
                        glib::timeout_add_local_once(Duration::from_secs(8), move || {
                            dirty.set(true);
                        });
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }
    glib::timeout_add_local(refresh_interval, move || {
        if tiles
            .borrow()
            .iter()
            .all(|tile| tile.button.root().is_none())
        {
            return glib::ControlFlow::Break;
        }
        let running = if refresh_tracker.is_some() {
            HashSet::new()
        } else {
            running_apps(desktop)
        };
        let current_revision = refresh_tracker.as_ref().map(KWinWindows::revision);
        let windows_changed =
            current_revision.is_some() && current_revision != last_synced_revision;
        // A pin was added or removed from the Dock menu: re-check every tile.
        let pins_changed = refresh_pin_state.dirty.replace(false);
        let current_preferences = crate::preferences::current();
        // The "minimise into icon" switch was flipped: re-evaluate the tiles.
        let minimize_changed =
            current_preferences.minimize_into_icon != applied_minimize_into_icon;
        let snapshot = if windows_changed || pins_changed || minimize_changed {
            refresh_tracker.as_ref().map(KWinWindows::snapshot)
        } else {
            None
        };
        let mut layout_changed = false;
        if current_preferences.dock_auto_hide != applied_auto_hide {
            if current_preferences.dock_auto_hide {
                refresh_hide_generation.set(refresh_hide_generation.get().wrapping_add(1));
                crate::animation::allow_for(500);
                refresh_animate_layout(320);
                refresh_revealer.set_reveal_child(false);
                let window = refresh_window_for_preferences.clone();
                let monitor = refresh_monitor_for_preferences.clone();
                let reposition = Rc::clone(&refresh_reposition_for_preferences);
                glib::timeout_add_local(Duration::from_millis(250), move || {
                    if let Some(window) = window.upgrade() {
                        resize_window_to_content(&window, &monitor);
                        reposition();
                    }
                    glib::ControlFlow::Break
                });
            } else {
                refresh_show_dock();
            }
            applied_auto_hide = current_preferences.dock_auto_hide;
        }
        if windows_changed {
            last_synced_revision = current_revision;
        }
        if windows_changed || pins_changed {
            if let (Some(snapshot), Some(tracker)) = (&snapshot, &refresh_tracker) {
                if sync_transient_tiles(
                    snapshot,
                    &mut tiles.borrow_mut(),
                    &refresh_row,
                    &refresh_display,
                    Some(tracker.clone()),
                    Rc::clone(&refresh_launching),
                    &refresh_pin_state,
                    &refresh_hover,
                ) {
                    for tile in tiles.borrow().iter() {
                        tile.button.set_size_request(
                            tile_size(&refresh_monitor, refresh_displayed_size.get()),
                            tile_size(&refresh_monitor, refresh_displayed_size.get()),
                        );
                        if let Some(icon) = &tile.icon {
                            icon.set_icon_size(
                                tile_size(&refresh_monitor, refresh_displayed_size.get()) - 4,
                            );
                        }
                    }
                    if let Some(window) = refresh_window.upgrade() {
                        resize_window_to_content(&window, &refresh_monitor);
                    }
                    layout_changed = true;
                }
            }
        }
        if let Some(snapshot) = &snapshot {
            let icon_size = tile_size(&refresh_monitor, refresh_displayed_size.get());
            if sync_minimized_tiles(
                snapshot,
                !current_preferences.minimize_into_icon,
                &tiles.borrow(),
                &mut refresh_minimized_tiles.borrow_mut(),
                &refresh_minimized_row,
                &refresh_display,
                &refresh_hover,
                icon_size,
                &thumbnails,
                &*refresh_animate_layout,
                &refresh_settle,
            ) {
                layout_changed = true;
            }
            let live: HashSet<&str> = snapshot
                .windows
                .iter()
                .map(|window| window.window_id.as_str())
                .collect();
            thumbnails.retain(&live);
        }
        // Real thumbnails: show finished captures, and keep capturing visible
        // windows so a window's picture is fresh when it gets minimised (KWin
        // cannot capture a window once it is minimised).
        let minimised_ids: HashSet<String> = refresh_minimized_tiles
            .borrow()
            .iter()
            .map(|tile| tile.window_id.clone())
            .collect();
        let updated = thumbnails.poll(&minimised_ids);
        if !updated.is_empty() {
            let icon_size = tile_size(&refresh_monitor, refresh_displayed_size.get());
            for tile in refresh_minimized_tiles.borrow().iter() {
                if updated.contains(&tile.window_id) {
                    if let Some(texture) = thumbnails.texture(&tile.window_id) {
                        if set_minimized_thumbnail(tile, &texture) {
                            size_minimized_tile(tile, icon_size);
                            layout_changed = true;
                        }
                    }
                }
            }
        }
        if !current_preferences.minimize_into_icon
            && last_thumbnail_capture
                .map_or(true, |at| at.elapsed() >= Duration::from_millis(1500))
        {
            if let Some(tracker) = &refresh_tracker {
                last_thumbnail_capture = Some(Instant::now());
                let windows = tracker.snapshot().windows;
                // One line whenever the picture of what could be captured changes.
                let summary = (
                    windows.iter().filter(|w| !w.minimized && w.is_trackable_app()).count(),
                    windows.iter().filter(|w| w.minimized && w.is_trackable_app()).count(),
                );
                if logged_thumbnail_summary != Some(summary) {
                    eprintln!(
                        "[thumbnail] switch is off; visible windows={} minimised windows={}",
                        summary.0, summary.1
                    );
                    logged_thumbnail_summary = Some(summary);
                }
                let visible = |window: &&kwin_windows::AppWindow| {
                    !window.minimized && !window.window_id.is_empty() && window.is_trackable_app()
                };
                // The focused window changes most, so refresh it first.
                for window in windows.iter().filter(visible).filter(|w| w.focused) {
                    thumbnails.request(&window.window_id);
                }
                for window in windows
                    .iter()
                    .filter(visible)
                    .filter(|w| !thumbnails.has_texture(&w.window_id))
                {
                    thumbnails.request(&window.window_id);
                }
            }
        }
        applied_minimize_into_icon = current_preferences.minimize_into_icon;
        if let Some(snapshot) = &snapshot {
            if last_logged_revision != Some(snapshot.revision) {
                let mut groups = std::collections::BTreeMap::<
                    String,
                    (usize, std::collections::BTreeSet<String>),
                >::new();
                for window in snapshot
                    .windows
                    .iter()
                    .filter(|window| window.is_trackable_app())
                {
                    let matched_entries: Vec<_> = tiles
                        .borrow()
                        .iter()
                        .filter(|tile| window.matches(&tile.app_aliases))
                        .map(|tile| tile.entry.name.clone())
                        .collect();
                    let identity = window.resolved_identity();
                    eprintln!("[dock-track-debug][window-dock-match] id={:?} identity={identity:?} matched_entries={matched_entries:?}", window.window_id);
                    let group = groups
                        .entry(identity)
                        .or_insert_with(|| (0, Default::default()));
                    group.0 += 1;
                    group.1.extend(matched_entries);
                }
                eprintln!("[dock-track-debug][grouped-dock-apps] applications={groups:?}");
                last_logged_revision = Some(snapshot.revision);
            }
        }
        if refresh_count == 0 {
            eprintln!(
                "[dock-track-debug][ui-refresh-start] tracker_available={} snapshot_window_count={} refresh_ms={}",
                snapshot.is_some(),
                snapshot.as_ref().map_or(0, |snapshot| snapshot.windows.len()),
                refresh_interval.as_millis()
            );
        }
        refresh_count += 1;
        if windows_changed || pins_changed || refresh_tracker.is_none() {
            let pins = refresh_pins.borrow();
            for tile in tiles.borrow().iter() {
                let (is_running, is_focused, all_minimized) = if let Some(snapshot) = &snapshot {
                    kwin_windows::matching_window_state(&tile.app_aliases, snapshot)
                } else {
                    (
                        tile.program
                            .as_deref()
                            .is_some_and(|program| app_is_running(program, &running)),
                        false,
                        false,
                    )
                };
                if is_running {
                    // The app has opened: stop its launch bounce.
                    if let Some(icon) = &tile.icon {
                        icon.stop_bounce();
                    }
                }
                let is_pinned = pins.contains(&tile.command);
                let should_reveal = is_pinned || is_running || tile.always_open;
                let state = (
                    is_running,
                    is_focused,
                    all_minimized,
                    is_pinned,
                    should_reveal,
                );
                if tile.revealer.reveals_child() != should_reveal {
                    if should_reveal && started_at.elapsed() < Duration::from_secs(3) {
                        refresh_animate_blur(260);
                        tile.revealer.set_visible(true);
                        tile.revealer.set_reveal_child(true);
                    } else if should_reveal {
                        // An app that just opened and isn't pinned: the Dock
                        // expands smoothly with its icon, then the icon
                        // bounces. Pinning an app that isn't running only
                        // expands.
                        expand_tile(
                            tile,
                            &*refresh_animate_layout,
                            is_running.then_some(Duration::ZERO),
                        );
                    } else {
                        // Removing a tile: the dock shrinks to fit straight away.
                        refresh_animate_blur(220);
                        tile.revealer.set_reveal_child(false);
                        let revealer = tile.revealer.downgrade();
                        let window = refresh_window.clone();
                        let monitor = refresh_monitor.clone();
                        let reposition = Rc::clone(&refresh_reposition);
                        glib::timeout_add_local_once(Duration::from_millis(200), move || {
                            let Some(revealer) = revealer.upgrade() else {
                                return;
                            };
                            if revealer.reveals_child() {
                                return;
                            }
                            revealer.set_visible(false);
                            if let Some(window) = window.upgrade() {
                                resize_window_to_content(&window, &monitor);
                            }
                            reposition();
                        });
                    }
                    layout_changed = true;
                }
                let is_open = tile.always_open || is_running;
                tile.indicator.set_opacity(if is_open { 1.0 } else { 0.0 });
                if is_focused {
                    tile.button.add_css_class("dock-item-focused");
                } else {
                    tile.button.remove_css_class("dock-item-focused");
                }
                if all_minimized {
                    tile.button.add_css_class("dock-item-minimized");
                } else {
                    tile.button.remove_css_class("dock-item-minimized");
                }
                let mut last_logged = refresh_log_state.borrow_mut();
                if last_logged.get(&tile.command) != Some(&state) {
                    let matched_windows: Vec<_> = snapshot
                        .as_ref()
                        .map(|snapshot| {
                            snapshot
                                .windows
                                .iter()
                                .filter(|window| window.matches(&tile.app_aliases))
                                .collect()
                        })
                        .unwrap_or_default();
                    let groups: HashSet<_> = matched_windows
                        .iter()
                        .map(|window| window.group_key())
                        .collect();
                    eprintln!(
                    "[dock-track-debug][entry-match-ui-applied] entry={:?} command={:?} aliases={:?} matched_windows={} app_groups={} windows={matched_windows:?} running={is_running} focused={is_focused} all_minimized={all_minimized} pinned={is_pinned} visible={} indicator_opacity={} css_focused={} css_minimized={}",
                    tile.entry.name,
                    tile.command,
                    tile.app_aliases,
                    matched_windows.len(),
                    groups.len(),
                    tile.revealer.reveals_child(),
                    tile.indicator.opacity(),
                    tile.button.has_css_class("dock-item-focused"),
                    tile.button.has_css_class("dock-item-minimized")
                );
                    last_logged.insert(tile.command.clone(), state);
                }
            }
        }
        refresh_launching
            .borrow_mut()
            .retain(|_, started| started.elapsed() < Duration::from_secs(6));
        if layout_changed {
            refresh_reposition();
            let window = refresh_window.clone();
            let monitor = refresh_monitor.clone();
            let reposition = Rc::clone(&refresh_reposition);
            glib::timeout_add_local(Duration::from_millis(190), move || {
                if let Some(window) = window.upgrade() {
                    resize_window_to_content(&window, &monitor);
                    reposition();
                }
                glib::ControlFlow::Break
            });
        }
        glib::ControlFlow::Continue
    });
    window
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kwin_windows::AppWindow;

    #[test]
    fn existing_window_is_activated_and_empty_snapshot_launches() {
        let snapshot = WindowSnapshot {
            windows: vec![AppWindow {
                window_id: "{konsole-window}".to_string(),
                title: "Konsole — shell".to_string(),
                desktop_file: "org.kde.konsole.desktop".to_string(),
                resource_class: "konsole".to_string(),
                resource_name: "konsole".to_string(),
                pid: 100,
                minimized: true,
                focused: false,
                special: false,
            }],
            ..WindowSnapshot::default()
        };
        let aliases = vec!["konsole".to_string()];
        assert!(should_activate_existing(
            Some("konsole"),
            &aliases,
            Some(&snapshot)
        ));
        assert!(!should_activate_existing(
            Some("konsole"),
            &aliases,
            Some(&WindowSnapshot::default())
        ));
    }
}
