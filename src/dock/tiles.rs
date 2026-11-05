//! Dock tile structures, context menus, and tile creation.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, Entry};
use crate::context_menu::ContextMenu;
use crate::kwin_windows::{self, KWinWindows, WindowSnapshot};

use super::apps::{app_is_running, running_process_names};
use super::icon::{icon_for, picture_for, RoundedIcon};
use super::launcher::{launch, launch_entry, program_name, settings_window_action};
use super::windows::{run_active_window_action, run_window_action};

#[derive(Clone)]
pub(crate) struct HoverLabel {
    pub(crate) popover: gtk::Popover,
    pub(crate) label: gtk::Label,
    pub(crate) parent: gtk::Widget,
    /// Bumped on every show/hide request so a delayed hide can tell it is stale.
    pub(crate) generation: Rc<std::cell::Cell<u64>>,
    pub(crate) current: Rc<RefCell<Option<glib::WeakRef<gtk::Button>>>>,
}

impl HoverLabel {
    pub(crate) fn new(parent: &impl IsA<gtk::Widget>) -> Self {
        let label = gtk::Label::new(None);
        label.add_css_class("lunar-system-menu");
        label.add_css_class("dock-label");

        let popover = gtk::Popover::new();
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(true);
        // Sit a little higher above the icon.
        popover.set_offset(0, -6);
        popover.set_autohide(false);
        popover.set_can_target(false);
        popover.set_focusable(false);
        popover.add_css_class("lunar-menu-popover");
        popover.add_css_class("dock-label-popover");
        popover.set_child(Some(&label));
        crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::bubble());
        popover.set_parent(parent);
        crate::context_menu::attach_blur_region(
            &popover,
            label.upcast_ref::<gtk::Widget>(),
            crate::liquid_glass::Material::bubble().blur_corner(),
        );

        Self {
            popover,
            label,
            parent: parent.as_ref().clone().upcast(),
            generation: Rc::new(std::cell::Cell::new(0)),
            current: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn bump(&self) -> u64 {
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        next
    }

    pub(crate) fn show_for(&self, tile: &gtk::Button, name: &str) {
        self.bump();
        self.label.set_text(name);
        if let Some(bounds) = tile.compute_bounds(&self.parent) {
            let rect = gdk::Rectangle::new(
                bounds.x().floor() as i32,
                bounds.y().floor() as i32,
                bounds.width().ceil() as i32,
                bounds.height().ceil() as i32,
            );
            // Re-pointing an already open popover just moves it.
            self.popover.set_pointing_to(Some(&rect));
        }
        *self.current.borrow_mut() = Some(tile.downgrade());
        self.popover.popup();
    }

    /// Keeps the open bubble anchored to the current tile while magnification
    /// changes the tile's size and position.
    pub(crate) fn refresh_position(&self) {
        let Some(tile) = self
            .current
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
        else {
            return;
        };
        if let Some(bounds) = tile.compute_bounds(&self.parent) {
            let rect = gdk::Rectangle::new(
                bounds.x().floor() as i32,
                bounds.y().floor() as i32,
                bounds.width().ceil() as i32,
                bounds.height().ceil() as i32,
            );
            self.popover.set_pointing_to(Some(&rect));
        }
    }

    pub(crate) fn hide_now(&self) {
        self.bump();
        *self.current.borrow_mut() = None;
        self.popover.popdown();
    }

    /// Hides after a moment unless the pointer lands on another icon first.
    pub(crate) fn hide_soon(&self) {
        let token = self.bump();
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(60), move || {
            if this.generation.get() == token {
                this.hide_now();
            }
        });
    }

    pub(crate) fn attach(&self, tile: &gtk::Button, name: &str) {
        let motion = gtk::EventControllerMotion::new();
        {
            let this = self.clone();
            let name = name.to_string();
            let weak_tile = tile.downgrade();
            motion.connect_enter(move |_, _, _| {
                if let Some(tile) = weak_tile.upgrade() {
                    this.show_for(&tile, &name);
                }
            });
        }
        {
            let this = self.clone();
            motion.connect_leave(move |_| this.hide_soon());
        }
        tile.add_controller(motion);

        // Clicking (launching, or opening the right-click menu) dismisses it.
        let press = gtk::GestureClick::new();
        press.set_button(0);
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let this = self.clone();
            press.connect_pressed(move |_, _, _, _| this.hide_now());
        }
        tile.add_controller(press);

        // Never leave the bubble behind when its tile disappears.
        let this = self.clone();
        tile.connect_unmap(move |tile| {
            let is_current = this
                .current
                .borrow()
                .as_ref()
                .and_then(glib::WeakRef::upgrade)
                .is_some_and(|current| &current == tile);
            if is_current {
                this.hide_now();
            }
        });
    }
}

/// Which commands are pinned to the Dock, shared by every tile's menu and the
/// refresh loop. Changes are saved straight away.
#[derive(Clone)]
pub(crate) struct PinState {
    pub(crate) pins: Rc<RefCell<HashSet<String>>>,
    /// Pinned apps that are not in dock.toml; stored beside the pins.
    pub(crate) extras: Rc<RefCell<Vec<Entry>>>,
    /// Commands that come from dock.toml.
    pub(crate) configured: Rc<HashSet<String>>,
    /// Set when a pin changed so the refresh loop re-evaluates the tiles.
    pub(crate) dirty: Rc<std::cell::Cell<bool>>,
}

impl PinState {
    pub(crate) fn is_pinned(&self, command: &str) -> bool {
        self.pins.borrow().contains(command)
    }

    pub(crate) fn toggle(&self, entry: &Entry) {
        let now_pinned = {
            let mut pins = self.pins.borrow_mut();
            if pins.remove(&entry.command) {
                false
            } else {
                pins.insert(entry.command.clone());
                true
            }
        };
        {
            let mut extras = self.extras.borrow_mut();
            extras.retain(|extra| extra.command != entry.command);
            if now_pinned && !self.configured.contains(&entry.command) {
                extras.push(entry.clone());
            }
        }
        if let Err(error) = config::save_pins(&self.pins.borrow(), &self.extras.borrow()) {
            eprintln!("lunar-desktop: could not save the Dock pins: {error}");
        }
        self.dirty.set(true);
    }
}

/// Replaces the text of a menu button created by `ContextMenu::append_action`.
pub(crate) fn set_item_label(button: &gtk::Button, text: &str) {
    if let Some(label) = button
        .child()
        .and_then(|child| child.first_child())
        .and_then(|child| child.downcast::<gtk::Label>().ok())
    {
        label.set_text(text);
    }
}

/// Right-click menu:
///
/// ```text
/// Options >  Add to Dock / Remove from Dock
/// ─────────
/// Open
/// Hide
/// Quit
/// ```
pub(crate) fn context_menu(
    tile: &gtk::Button,
    entry: &Entry,
    pin_state: PinState,
    always_open: bool,
    is_running: Rc<dyn Fn() -> bool>,
    launch_bounce: Rc<dyn Fn()>,
) {
    let program = program_name(&entry.command);
    let menu = ContextMenu::new(gtk::PositionType::Top, &["dock-context-menu"]);
    // Opens above the icon with a small arrow pointing down at it.
    menu.set_arrow(true);
    menu.add_popover_class("dock-context-popover");

    // Options > Add to / Remove from Dock
    let pin_item: Rc<RefCell<Option<gtk::Button>>> = Rc::new(RefCell::new(None));
    {
        let parent_menu = menu.clone();
        let pin_item = Rc::clone(&pin_item);
        let pin_state = pin_state.clone();
        let pin_entry = entry.clone();
        menu.append_submenu("Options", &["dock-context-submenu"], move |submenu| {
            let button = submenu.append_action("Add to Dock", !always_open, move || {
                pin_state.toggle(&pin_entry);
                parent_menu.popdown();
            });
            *pin_item.borrow_mut() = Some(button);
        });
    }

    menu.append_separator();

    let open_entry = entry.clone();
    let open_program = program.clone();
    let open_running = Rc::clone(&is_running);
    menu.append_action("Open", true, move || {
        if open_running() {
            if let Some(program) = &open_program {
                run_window_action(program.clone(), "activate");
                return;
            }
        }
        launch_bounce();
        launch_entry(&open_entry);
    });

    let hide_program = program.clone();
    let hide_button = menu.append_action("Hide", false, move || {
        if let Some(program) = &hide_program {
            run_window_action(program.clone(), "hide");
        }
    });

    let quit_program = program.clone();
    let quit_button = menu.append_action("Quit", false, move || {
        if let Some(program) = &quit_program {
            run_window_action(program.clone(), "quit");
        }
    });

    menu.set_parent(tile);

    let command = entry.command.clone();
    let can_control = program.is_some();
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    let menu_for_click = menu.clone();
    gesture.connect_pressed(move |gesture, _, _, _| {
        // Refresh what the menu offers for the app's current state.
        let running = is_running();
        quit_button.set_sensitive(running && can_control);
        hide_button.set_sensitive(running && can_control);
        if let Some(button) = pin_item.borrow().as_ref() {
            let label = if pin_state.is_pinned(&command) {
                "Remove from Dock"
            } else {
                "Add to Dock"
            };
            set_item_label(button, label);
        }
        menu_for_click.popup_on_parent();
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    tile.add_controller(gesture);
}

pub(crate) struct DockTile {
    pub(crate) button: gtk::Button,
    pub(crate) revealer: gtk::Revealer,
    pub(crate) icon: Option<RoundedIcon>,
    pub(crate) indicator: gtk::Widget,
    pub(crate) program: Option<String>,
    pub(crate) app_aliases: Vec<String>,
    pub(crate) command: String,
    pub(crate) always_open: bool,
    pub(crate) dynamic_identity: Option<String>,
    pub(crate) entry: Entry,
    /// Starts the launch bounce on this tile's icon.
    pub(crate) launch_bounce: Rc<dyn Fn()>,
    /// Like `launch_bounce`, but keeps hopping for at most the given time
    /// (`Duration::ZERO` = a single hop).
    pub(crate) bounce_for: Rc<dyn Fn(Duration)>,
}


pub(crate) fn app_aliases(program: Option<&str>) -> Vec<String> {
    let Some(program) = program else {
        return Vec::new();
    };
    kwin_windows::aliases_for_program(program)
}

pub(crate) fn should_activate_existing(
    program: Option<&str>,
    aliases: &[String],
    snapshot: Option<&WindowSnapshot>,
) -> bool {
    if let Some(snapshot) = snapshot {
        kwin_windows::matching_window_state(aliases, snapshot).0
    } else {
        program.is_some_and(|program| app_is_running(program, &running_process_names()))
    }
}

pub(crate) fn create_tile(
    entry: Entry,
    display: &gdk::Display,
    tracker: Option<KWinWindows>,
    launching: Rc<RefCell<HashMap<String, Instant>>>,
    aliases: Vec<String>,
    pinned: bool,
    always_open: bool,
    initially_running: bool,
    dynamic_identity: Option<String>,
    pin_state: PinState,
    hover: &HoverLabel,
) -> DockTile {
    let temporary = dynamic_identity.is_some();
    let tile = gtk::Button::builder()
        .css_classes(["dock-item"])
        .focusable(false)
        .build();
    hover.attach(&tile, &entry.name);

    let icon = icon_for(&entry, display);
    let picture = icon.as_ref().map(|icon| icon.picture.clone());
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    let rounded_icon = match &picture {
        Some(picture) => {
            tile.add_css_class("dock-icon");
            let rounded_icon = RoundedIcon::new(picture);
            content.append(&rounded_icon);
            Some(rounded_icon)
        }
        None => {
            let letter = entry
                .name
                .chars()
                .next()
                .map_or("?".to_string(), |c| c.to_uppercase().to_string());
            tile.add_css_class("dock-placeholder");
            content.append(&gtk::Label::new(Some(&letter)));
            None
        }
    };

    let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    indicator.set_size_request(4, 4);
    indicator.set_halign(gtk::Align::Center);
    indicator.add_css_class("dock-running-indicator");
    indicator.set_opacity(if always_open || initially_running {
        1.0
    } else {
        0.0
    });
    content.append(&indicator);
    tile.set_child(Some(&content));

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
    // Keep the tile's bottom edge fixed so magnification rises above the shelf.
    revealer.set_valign(gtk::Align::End);
    revealer.set_transition_duration(0);
    revealer.set_child(Some(&tile));
    revealer.set_reveal_child(pinned || initially_running || always_open);
    revealer.set_visible(pinned || initially_running || always_open);
    revealer.set_transition_duration(180);

    let program = program_name(&entry.command);
    let click_entry = entry.clone();
    let click_launching = Rc::clone(&launching);
    let menu_tracker = tracker.clone();
    let click_tracker = tracker;
    let click_program = program.clone();
    let click_aliases = aliases.clone();
    let click_pins = pin_state.clone();
    // Hops the icon for at most `max`, letting it rise above the Revealer.
    let bounce_for: Rc<dyn Fn(Duration)> = {
        let icon = rounded_icon.clone();
        let revealer = revealer.downgrade();
        Rc::new(move |max| {
            if let (Some(icon), Some(revealer)) = (&icon, revealer.upgrade()) {
                // The Revealer clips its child; let the icon rise above it.
                revealer.set_overflow(gtk::Overflow::Visible);
                let revealer = revealer.downgrade();
                icon.start_bounce(max, move || {
                    if let Some(revealer) = revealer.upgrade() {
                        revealer.set_overflow(gtk::Overflow::Hidden);
                    }
                });
            }
        })
    };
    // Shared by a left click and the right-click menu's "Open".
    let launch_bounce: Rc<dyn Fn()> = {
        let bounce_for = Rc::clone(&bounce_for);
        Rc::new(move || bounce_for(Duration::from_secs(6)))
    };
    let click_bounce = Rc::clone(&launch_bounce);
    let click_gesture = gtk::GestureClick::new();
    click_gesture.set_button(1);
    click_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    click_gesture.connect_pressed(|gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
    });
    click_gesture.connect_released(move |_, _, _, _| {
        let snapshot = click_tracker.as_ref().map(KWinWindows::snapshot);
        if should_activate_existing(click_program.as_deref(), &click_aliases, snapshot.as_ref()) {
            if let Some(program) = &click_program {
                // With "minimise into icon" off, minimised windows have their
                // own tiles; the app icon only raises windows that are showing.
                let action = if crate::preferences::current().minimize_into_icon {
                    "activate"
                } else {
                    "activate-visible"
                };
                run_window_action(program.clone(), action);
            }
            return;
        }
        // A running app that was added to the Dock can be launched again.
        if temporary && !click_pins.is_pinned(&click_entry.command) {
            return;
        }
        let now = Instant::now();
        let mut launching = click_launching.borrow_mut();
        if launching
            .get(&click_entry.command)
            .is_some_and(|started| now.duration_since(*started) < Duration::from_secs(6))
        {
            return;
        }
        launching.insert(click_entry.command.clone(), now);
        drop(launching);
        // Only a real launch bounces; activating an open app does not.
        click_bounce();
        launch_entry(&click_entry);
    });
    tile.add_controller(click_gesture);
    let running_program = program.clone();
    let running_aliases = aliases.clone();
    let is_running: Rc<dyn Fn() -> bool> = Rc::new(move || {
        let snapshot = menu_tracker.as_ref().map(KWinWindows::snapshot);
        should_activate_existing(running_program.as_deref(), &running_aliases, snapshot.as_ref())
    });
    context_menu(&tile, &entry, pin_state, always_open, is_running, Rc::clone(&launch_bounce));

    DockTile {
        button: tile,
        revealer,
        icon: rounded_icon,
        indicator: indicator.upcast(),
        program,
        app_aliases: aliases,
        command: entry.command.clone(),
        always_open,
        dynamic_identity,
        entry,
        launch_bounce,
        bounce_for,
    }
}

pub(crate) fn create_trash_item() -> (gtk::Button, RoundedIcon) {
    let button = gtk::Button::builder()
        .css_classes(["dock-item", "dock-icon"])
        .focusable(false)
        .build();
    let icon_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/dock-icons/trash.png");
    let picture = match super::icon::load_icon_texture(&icon_path) {
        Ok(texture) => picture_for(&texture),
        Err(error) => {
            eprintln!("lunar-desktop: supplied Trash icon failed to load: {error}");
            gtk::Picture::for_filename(icon_path)
        }
    };
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    let rounded_icon = RoundedIcon::new(&picture);
    rounded_icon.add_css_class("dock-trash-icon");
    button.set_child(Some(&rounded_icon));
    button.connect_clicked(|_| launch("kioclient exec trash:/"));
    (button, rounded_icon)
}
