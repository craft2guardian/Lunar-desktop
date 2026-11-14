//! Launchpad: a dark capsule (title / search) floating above a glass panel with
//! scrolling category chips and a grid of every installed application. The top
//! row holds the apps launched most often from here; below it everything is
//! listed A-Z. Opened from its Dock tile (the built-in `lunar:launchpad`
//! command, placed right after Finder).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// The Dock command that opens the Launchpad (see `config.rs` and `dock.toml`).
pub const COMMAND: &str = crate::config::LAUNCHPAD_COMMAND;

const TITLE: &str = "Launchpad";
const PANEL_WIDTH: i32 = 760;
const PANEL_HEIGHT: i32 = 500;
const PANEL_RADIUS: f32 = 30.0;
const PANEL_PADDING: i32 = 20;
const CAPSULE_WIDTH: i32 = 560;
const CAPSULE_HEIGHT: i32 = 76;
const GAP: i32 = 12;
/// Transparent margin around the window content, for the drop shadows.
const SHADOW_ROOM: i32 = 28;
const COLUMNS: i32 = 7;
const CELL_WIDTH: i32 = (PANEL_WIDTH - 2 * PANEL_PADDING) / COLUMNS;
const ICON_SIZE: i32 = 52;

/// The category chips, in order. The last group also collects everything that
/// does not belong to another one.
const GROUPS: [&str; 8] = [
    "Productivity & Finance",
    "Utilities",
    "Social",
    "Entertainment",
    "Creativity",
    "Games",
    "Information & Reading",
    "Other",
];

thread_local! {
    /// When the panel last closed. Clicking the Dock tile moves focus away
    /// (which closes the panel) just before the click is handled; without this
    /// the same click would immediately open it again.
    static LAST_CLOSED: Cell<Option<Instant>> = const { Cell::new(None) };
}

struct AppItem {
    app: gio::AppInfo,
    name: String,
    group: usize,
}

/// Opens the Launchpad, or closes it if it is already open.
pub fn toggle() {
    let Some(app) = gio::Application::default().and_then(|app| app.downcast::<gtk::Application>().ok())
    else {
        return;
    };
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|window| window.title().as_deref() == Some(TITLE))
    {
        window.close();
        return;
    }
    let just_closed = LAST_CLOSED
        .with(Cell::get)
        .is_some_and(|closed| closed.elapsed() < Duration::from_millis(350));
    if just_closed {
        return;
    }
    open(&app);
}

fn open(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(TITLE)
        .resizable(false)
        .decorated(false)
        .css_classes(["lunar-launchpad"])
        .build();

    let items = Rc::new(load_apps());
    let usage = Rc::new(RefCell::new(load_usage()));

    // ---- capsule: icon, search field that doubles as the title, menu dots ----
    let capsule = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    capsule.add_css_class("launchpad-capsule");
    capsule.set_size_request(CAPSULE_WIDTH, CAPSULE_HEIGHT);
    // The App Store-style glyph (white on transparent; tinted by CSS opacity).
    let glyph = gtk::gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../assets/launchpad-glyph.png"
    )))
    .ok();
    let capsule_icon = match &glyph {
        Some(texture) => gtk::Image::from_paintable(Some(texture)),
        None => gtk::Image::from_icon_name("applications-all-symbolic"),
    };
    capsule_icon.set_pixel_size(30);
    capsule_icon.add_css_class("launchpad-capsule-icon");
    let search = gtk::Entry::new();
    search.set_placeholder_text(Some("Applications"));
    search.set_has_frame(false);
    search.set_hexpand(true);
    search.set_valign(gtk::Align::Center);
    search.add_css_class("launchpad-search");
    let more = gtk::Label::new(Some("•••"));
    more.add_css_class("launchpad-more");
    capsule.append(&capsule_icon);
    capsule.append(&search);
    capsule.append(&more);

    // ---- category chips (scroll sideways when they do not all fit) ----
    let chips = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    chips.add_css_class("launchpad-chips");
    let chip_buttons: Vec<gtk::Button> = GROUPS
        .iter()
        .map(|name| {
            let button = gtk::Button::with_label(name);
            button.set_has_frame(false);
            button.add_css_class("launchpad-chip");
            chips.append(&button);
            button
        })
        .collect();
    let chip_scroller = gtk::ScrolledWindow::new();
    chip_scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    chip_scroller.set_child(Some(&chips));
    chip_scroller.add_css_class("launchpad-chip-scroller");

    // ---- scrolling app list ----
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("launchpad-list");
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    scroller.add_css_class("launchpad-scroller");

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("launchpad-panel");
    panel.set_size_request(PANEL_WIDTH, PANEL_HEIGHT);
    panel.set_overflow(gtk::Overflow::Hidden);
    panel.append(&chip_scroller);
    panel.append(&scroller);

    let glass = crate::liquid_glass::GlassSurface::wrap(
        &panel,
        borderless(
            crate::liquid_glass::Material::clear_card()
                .with_radius(crate::glass_config::Radius::Fixed(PANEL_RADIUS)),
        ),
    );
    glass.set_size_request(PANEL_WIDTH, PANEL_HEIGHT);

    let root = gtk::Box::new(gtk::Orientation::Vertical, GAP);
    root.add_css_class("launchpad-root");
    // Transparent room around the surfaces for their soft shadows.
    root.set_margin_top(SHADOW_ROOM);
    root.set_margin_bottom(SHADOW_ROOM);
    root.set_margin_start(SHADOW_ROOM);
    root.set_margin_end(SHADOW_ROOM);
    // The capsule gets the same liquid glass as everything else.
    let capsule_glass = crate::liquid_glass::GlassSurface::wrap(
        &capsule,
        borderless(crate::liquid_glass::Material::clear_capsule()),
    );
    capsule_glass.set_size_request(CAPSULE_WIDTH, CAPSULE_HEIGHT);
    capsule_glass.set_halign(gtk::Align::Center);
    root.append(&capsule_glass);
    root.append(&glass);
    window.set_child(Some(&root));

    // ---- behaviour ----
    // The category chosen with a chip (clicking it again clears it), and the
    // apps currently shown (for Enter).
    let selected: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let shown: Rc<RefCell<Vec<gio::AppInfo>>> = Rc::new(RefCell::new(Vec::new()));

    let launch_app: Rc<dyn Fn(&gio::AppInfo)> = {
        let window = window.downgrade();
        let usage = Rc::clone(&usage);
        Rc::new(move |app: &gio::AppInfo| {
            if !crate::dock::launch_app_detached(app) {
                if let Err(error) = app.launch(&[], None::<&gio::AppLaunchContext>) {
                    eprintln!("lunar-desktop: could not launch {}: {error}", app.name());
                }
            }
            if let Some(id) = app.id() {
                let mut usage = usage.borrow_mut();
                *usage.entry(id.to_string()).or_insert(0) += 1;
                save_usage(&usage);
            }
            if let Some(window) = window.upgrade() {
                window.close();
            }
        })
    };

    let populate: Rc<dyn Fn(&str)> = {
        let list = list.clone();
        let items = Rc::clone(&items);
        let usage = Rc::clone(&usage);
        let selected = Rc::clone(&selected);
        let shown = Rc::clone(&shown);
        let launch_app = Rc::clone(&launch_app);
        let scroller = scroller.clone();
        Rc::new(move |query: &str| {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let query = query.trim().to_lowercase();
            let group = selected.get();
            let mut shown = shown.borrow_mut();
            shown.clear();

            let matching: Vec<&AppItem> = items
                .iter()
                .filter(|item| group.map_or(true, |group| item.group == group))
                .filter(|item| query.is_empty() || item.name.to_lowercase().contains(&query))
                .collect();

            // The most-used apps get their own row on top (not while searching
            // or browsing a single category).
            if query.is_empty() && group.is_none() {
                let usage = usage.borrow();
                let mut frequent: Vec<(&AppItem, u32)> = matching
                    .iter()
                    .filter_map(|item| {
                        let count = *usage.get(item.app.id()?.as_str())?;
                        (count > 0).then_some((*item, count))
                    })
                    .collect();
                frequent.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
                frequent.truncate(COLUMNS as usize);
                if !frequent.is_empty() {
                    let top: Vec<&AppItem> = frequent.into_iter().map(|(item, _)| item).collect();
                    list.append(&grid_of(&top, &launch_app, &mut shown));
                    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
                    separator.add_css_class("launchpad-separator");
                    list.append(&separator);
                }
            }

            list.append(&grid_of(&matching, &launch_app, &mut shown));
            scroller.vadjustment().set_value(0.0);
        })
    };
    populate("");

    {
        let populate = Rc::clone(&populate);
        search.connect_changed(move |entry| populate(&entry.text()));
    }
    {
        let shown = Rc::clone(&shown);
        let launch_app = Rc::clone(&launch_app);
        search.connect_activate(move |_| {
            let first = shown.borrow().first().cloned();
            if let Some(app) = first {
                launch_app(&app);
            }
        });
    }
    for (group, button) in chip_buttons.iter().enumerate() {
        let populate = Rc::clone(&populate);
        let selected = Rc::clone(&selected);
        let search = search.clone();
        let chip_buttons = chip_buttons.clone();
        button.connect_clicked(move |_| {
            let now = if selected.get() == Some(group) { None } else { Some(group) };
            selected.set(now);
            for (index, other) in chip_buttons.iter().enumerate() {
                if now == Some(index) {
                    other.add_css_class("selected");
                } else {
                    other.remove_css_class("selected");
                }
            }
            populate(&search.text());
        });
    }

    // Escape closes the panel; so does moving focus to another window.
    let keys = gtk::EventControllerKey::new();
    {
        let window = window.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);

    let was_active = Rc::new(Cell::new(false));
    window.connect_is_active_notify(move |window| {
        if window.is_active() {
            was_active.set(true);
        } else if was_active.get() {
            window.close();
        }
    });
    window.connect_destroy(|_| LAST_CLOSED.with(|closed| closed.set(Some(Instant::now()))));

    crate::preferences::apply_dark_mode(&window);
    // Blur behind the capsule and the panel, but not the gap between them.
    crate::context_menu::attach_window_regions_blur(
        window.upcast_ref(),
        vec![
            (capsule_glass.clone().upcast(), CAPSULE_HEIGHT / 2),
            (glass.clone().upcast(), PANEL_RADIUS as i32),
        ],
    );
    window.present();
    search.grab_focus();
}

/// The Launchpad's glass: no hover/press response and no rim or specular
/// lines around the edge; a faint dark shadow separates it from the desktop.
fn borderless(mut material: crate::liquid_glass::Material) -> crate::liquid_glass::Material {
    use crate::glass_config::{Shadow, ShadowLayer};
    material.interactive = false;
    material.hover_clears = false;
    material.border.width = 0.0;
    material.border.opacity = 0.0;
    material.highlight.strength = 0.0;
    material.shadow = Shadow {
        intensity: 0.55,
        color: [0.0, 0.0, 0.02],
        ambient: ShadowLayer { alpha: 0.30, dx: 0.0, dy: 6.0, blur: 22.0, spread: 0.0 },
        contact: ShadowLayer { alpha: 0.0, dx: 0.0, dy: 0.0, blur: 1.0, spread: 0.0 },
    };
    material
}

/// A fixed-width 7-column grid of apps (rows with few apps keep their cell
/// size instead of stretching across the panel).
fn grid_of(
    items: &[&AppItem],
    launch_app: &Rc<dyn Fn(&gio::AppInfo)>,
    shown: &mut Vec<gio::AppInfo>,
) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(4);
    grid.set_halign(gtk::Align::Start);
    grid.add_css_class("launchpad-grid");
    for (index, item) in items.iter().enumerate() {
        let index = index as i32;
        grid.attach(&app_cell(&item.app, &item.name, launch_app), index % COLUMNS, index / COLUMNS, 1, 1);
        shown.push(item.app.clone());
    }
    grid
}

fn app_cell(app: &gio::AppInfo, name: &str, launch_app: &Rc<dyn Fn(&gio::AppInfo)>) -> gtk::Button {
    let icon = match app.icon() {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    icon.set_pixel_size(ICON_SIZE);
    icon.add_css_class("launchpad-app-icon");

    let label = gtk::Label::new(Some(name));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(12);
    label.add_css_class("launchpad-app-label");

    let content = gtk::Box::new(gtk::Orientation::Vertical, 5);
    content.set_halign(gtk::Align::Center);
    content.append(&icon);
    content.append(&label);

    let button = gtk::Button::new();
    button.set_child(Some(&content));
    button.set_has_frame(false);
    button.set_size_request(CELL_WIDTH, -1);
    button.add_css_class("launchpad-app");
    let app = app.clone();
    let launch_app = Rc::clone(launch_app);
    button.connect_clicked(move |_| launch_app(&app));
    button
}

/// Every installed, visible application, sorted by name, one per name.
fn load_apps() -> Vec<AppItem> {
    let mut seen = HashSet::new();
    let mut items: Vec<AppItem> = gio::AppInfo::all()
        .into_iter()
        .filter(|app| app.should_show())
        .filter_map(|app| {
            let name = app.name().to_string();
            seen.insert(name.to_lowercase()).then(|| {
                let group = group_of(&app);
                AppItem { app, name, group }
            })
        })
        .collect();
    items.sort_by_key(|item| item.name.to_lowercase());
    items
}

/// Maps a desktop entry's freedesktop categories onto the chip groups.
fn group_of(app: &gio::AppInfo) -> usize {
    let categories = crate::dock::desktop_file_for(app)
        .and_then(|path| {
            let file = glib::KeyFile::new();
            file.load_from_file(&path, glib::KeyFileFlags::NONE).ok()?;
            file.string("Desktop Entry", "Categories").ok()
        })
        .map(|categories| categories.to_string())
        .unwrap_or_default();
    let has = |wanted: &[&str]| categories.split(';').any(|category| wanted.contains(&category));

    if has(&["Game"]) {
        5
    } else if has(&[
        "InstantMessaging", "Chat", "IRCClient", "Email", "Feed", "VideoConference", "Telephony", "ContactManagement",
        "Social",
    ]) {
        2
    } else if has(&["Graphics", "2DGraphics", "3DGraphics", "VectorGraphics", "RasterGraphics", "Photography", "Publishing"]) {
        4
    } else if has(&["AudioVideo", "Audio", "Video", "Music", "Player", "Recorder", "Midi", "Mixer", "Sequencer", "TV"]) {
        3
    } else if has(&[
        "Education", "Science", "Literature", "News", "Documentation", "Viewer", "Maps", "Geography", "Dictionary",
        "Languages",
    ]) {
        6
    } else if has(&[
        "Office", "Finance", "ProjectManagement", "Calendar", "WordProcessor", "Spreadsheet", "Presentation", "Database",
        "Development",
    ]) {
        0
    } else if has(&[
        "Utility", "System", "Settings", "Accessibility", "Archiving", "FileTools", "FileManager", "TerminalEmulator",
        "Monitor", "Core",
    ]) {
        1
    } else {
        7
    }
}

// ---- launch counts (drive the top row) ----

fn usage_path() -> Option<std::path::PathBuf> {
    crate::preferences::config_dir().map(|dir| dir.join("launchpad-usage.txt"))
}

/// One `count<TAB>desktop-id` per line.
fn load_usage() -> HashMap<String, u32> {
    let Some(text) = usage_path().and_then(|path| std::fs::read_to_string(path).ok()) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (count, id) = line.split_once('\t')?;
            Some((id.to_string(), count.parse().ok()?))
        })
        .collect()
}

fn save_usage(usage: &HashMap<String, u32>) {
    let Some(path) = usage_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text: String = usage.iter().map(|(id, count)| format!("{count}\t{id}\n")).collect();
    let _ = std::fs::write(path, text);
}
