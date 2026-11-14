//! A lightweight System Settings window for shell configuration pages.
//!
//! Layout (like macOS): a translucent sidebar on the left that also holds the
//! window's close/minimise/maximise buttons, and a content area on the right
//! with a toolbar (back/forward + page title) above the page itself. There is
//! no separate title bar: the window draws its own frame ("client-side
//! decorations"), which is what lets the sidebar run all the way to the top.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const WINDOW_TITLE: &str = "System Settings";
/// The window is exactly this wide and can only be resized vertically.
const WINDOW_WIDTH: i32 = 720;
const WINDOW_MIN_HEIGHT: i32 = 420;
const SIDEBAR_WIDTH: i32 = 232;

unsafe extern "C" {
    /// Tells the Wayland compositor the window's size limits (0 = unlimited).
    fn lunar_toplevel_set_size_limits(
        surface: *mut gdk::ffi::GdkSurface,
        min_width: i32,
        min_height: i32,
        max_width: i32,
        max_height: i32,
    );
}

fn lock_window_width(window: &gtk::ApplicationWindow) {
    use gtk::glib::translate::ToGlibPtr;
    let Some(surface) = window.surface() else { return };
    unsafe {
        lunar_toplevel_set_size_limits(
            surface.to_glib_none().0,
            WINDOW_WIDTH,
            WINDOW_MIN_HEIGHT,
            WINDOW_WIDTH,
            0,
        );
    }
}

thread_local! {
    /// The open settings window's sidebar, so a page can be shown from outside.
    static SIDEBAR: RefCell<Option<glib::WeakRef<gtk::ListBox>>> = RefCell::new(None);
}

/// One sidebar entry and the page it shows.
struct Page {
    /// Name of the page inside the `gtk::Stack`.
    name: &'static str,
    title: &'static str,
    /// PNG bytes, compiled into the binary so the app works from any folder.
    icon: &'static [u8],
    dark_icon: &'static [u8],
    symbolic_icon: Option<&'static str>,
    /// Pages with the same number sit together; a gap separates different
    /// numbers, like the grouped sections in macOS System Settings.
    group: u8,
}

/// To add a page: add an entry here and a matching arm in `build_page`.
const PAGES: [Page; 8] = [
    Page {
        name: "wifi",
        title: "Wi-Fi",
        icon: include_bytes!("../assets/settings-icons/wifi.png"),
        dark_icon: include_bytes!("../assets/settings-icons/wifi.png"),
        symbolic_icon: None,
        group: 0,
    },
    Page {
        name: "bluetooth",
        title: "Bluetooth",
        icon: include_bytes!("../assets/settings-icons/bluetooth.png"),
        dark_icon: include_bytes!("../assets/settings-icons/bluetooth.png"),
        symbolic_icon: None,
        group: 0,
    },
    Page {
        name: "general",
        title: "General",
        icon: include_bytes!("../assets/settings-icons/general.png"),
        dark_icon: include_bytes!("../assets/settings-icons/general.png"),
        symbolic_icon: None,
        group: 1,
    },
    Page {
        name: "appearance",
        title: "Appearance",
        icon: &[],
        dark_icon: &[],
        symbolic_icon: Some("preferences-desktop-theme-symbolic"),
        group: 1,
    },
    Page {
        name: "desktop-dock",
        title: "Desktop & Dock",
        icon: include_bytes!("../assets/settings-icons/desktop-dock.png"),
        dark_icon: include_bytes!("../assets/settings-icons/desktop-dock.png"),
        symbolic_icon: None,
        group: 1,
    },
    Page {
        name: "displays",
        title: "Displays",
        icon: include_bytes!("../assets/settings-icons/displays.png"),
        dark_icon: include_bytes!("../assets/settings-icons/displays.png"),
        symbolic_icon: None,
        group: 1,
    },
    Page {
        name: "menu-bar",
        title: "Menu Bar",
        icon: include_bytes!("../assets/settings-icons/menu-bar.png"),
        dark_icon: include_bytes!("../assets/settings-icons/menu-bar.png"),
        symbolic_icon: None,
        group: 1,
    },
    Page {
        name: "wallpaper",
        title: "Wallpaper",
        icon: &[],
        dark_icon: &[],
        symbolic_icon: Some("preferences-desktop-wallpaper-symbolic"),
        group: 2,
    },
];

fn sidebar_row(page: &Page) -> gtk::ListBoxRow {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);

    let icons = if let Some(icon_name) = page.symbolic_icon {
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(20);
        // Same 24 px slot as the picture icons so the labels line up.
        icon.set_size_request(24, 24);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("settings-sidebar-icon");
        icon.upcast::<gtk::Widget>()
    } else {
        let icons = gtk::Overlay::new();
        icons.add_css_class("settings-sidebar-icon");
        icons.set_size_request(24, 24);
        icons.set_valign(gtk::Align::Center);
        let icon = gtk::Image::new();
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(page.icon)) {
            icon.set_paintable(Some(&texture));
        }
        icon.set_pixel_size(24);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("settings-icon-light");
        icons.set_child(Some(&icon));
        let dark_icon = gtk::Image::new();
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(page.dark_icon)) {
            dark_icon.set_paintable(Some(&texture));
        }
        dark_icon.set_pixel_size(24);
        dark_icon.set_halign(gtk::Align::Center);
        dark_icon.set_valign(gtk::Align::Center);
        dark_icon.add_css_class("settings-icon-dark");
        icons.add_overlay(&dark_icon);
        icons.upcast::<gtk::Widget>()
    };

    let label = gtk::Label::new(Some(page.title));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);

    content.append(&icons);
    content.append(&label);

    let row = gtk::ListBoxRow::new();
    row.add_css_class("settings-sidebar-row");
    row.set_child(Some(&content));
    row
}

/// The name macOS shows at the top of the sidebar: the account's full name,
/// or its login name when there is no full name.
fn account_name() -> String {
    let full = glib::real_name().to_string_lossy().trim().to_string();
    if !full.is_empty() && full != "Unknown" {
        return full;
    }
    glib::user_name().to_string_lossy().into_owned()
}

/// Round avatar, name and a caption, like the Apple ID block in System Settings.
fn account_block() -> gtk::Box {
    let name = account_name();
    let initial = name
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |c| c.to_uppercase().to_string());

    let avatar = gtk::Label::new(Some(&initial));
    avatar.add_css_class("settings-account-avatar");
    avatar.set_size_request(42, 42);
    avatar.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some(&name));
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.add_css_class("settings-account-name");
    let caption = gtk::Label::new(Some("Local Account"));
    caption.set_halign(gtk::Align::Start);
    caption.set_xalign(0.0);
    caption.add_css_class("settings-account-caption");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_valign(gtk::Align::Center);
    text.append(&title);
    text.append(&caption);

    let block = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    block.add_css_class("settings-account");
    block.append(&avatar);
    block.append(&text);
    block
}

/// One line inside a group: title (and optional subtitle) on the left, the
/// control on the right.
pub(crate) fn settings_row(title: &str, subtitle: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(title));
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("settings-row-title");
    text.append(&title);
    if let Some(subtitle) = subtitle {
        let subtitle = gtk::Label::new(Some(subtitle));
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.add_css_class("settings-row-subtitle");
        text.append(&subtitle);
    }

    control.set_valign(gtk::Align::Center);
    row.append(&text);
    row.append(control);
    row
}

/// A white rounded card holding `rows`, with hairlines between them.
pub(crate) fn settings_group(rows: &[gtk::Box]) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add_css_class("settings-group");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            let line = gtk::Separator::new(gtk::Orientation::Horizontal);
            line.add_css_class("settings-row-separator");
            group.append(&line);
        }
        group.append(row);
    }
    group
}

pub(crate) fn section_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.add_css_class("settings-section-heading");
    label
}

/// macOS pop-up button: the current value plus the blue up/down stepper.
/// The choice is only remembered by the button for now.
pub(crate) fn popup_button(options: &'static [&'static str], selected: usize) -> gtk::Button {
    let value = gtk::Label::new(Some(options[selected]));
    value.add_css_class("settings-popup-value");

    let up = gtk::Image::from_icon_name("pan-up-symbolic");
    let down = gtk::Image::from_icon_name("pan-down-symbolic");
    up.set_pixel_size(8);
    down.set_pixel_size(8);
    let stepper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stepper.add_css_class("settings-popup-stepper");
    stepper.set_valign(gtk::Align::Center);
    stepper.append(&up);
    stepper.append(&down);

    let contents = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    contents.append(&value);
    contents.append(&stepper);

    let button = gtk::Button::new();
    button.add_css_class("settings-popup");
    button.set_child(Some(&contents));

    let menu = crate::context_menu::ContextMenu::new(
        gtk::PositionType::Bottom,
        &["settings-popup-menu"],
    );
    menu.set_parent(&button);
    for option in options {
        let value = value.clone();
        menu.append_action(option, true, move || value.set_text(option));
    }
    button.connect_clicked(move |button| {
        menu.popup_at(button.width() / 2, button.height());
    });
    button
}

pub(crate) fn popup_row(title: &str, options: &'static [&'static str], selected: usize) -> gtk::Box {
    settings_row(title, None, &popup_button(options, selected))
}

pub(crate) fn switch_row(title: &str, subtitle: Option<&str>, active: bool) -> (gtk::Box, gtk::Switch) {
    let switch = gtk::Switch::new();
    switch.set_active(active);
    (settings_row(title, subtitle, &switch), switch)
}

/// A tappable row with a leading icon, title, and trailing chevron — used on
/// the General page for About, Software Update, Storage, and similar links.
fn navigation_row(title: &str, icon_name: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    row.add_css_class("settings-nav-row");

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(22);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("settings-nav-icon");

    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    label.set_xalign(0.0);
    label.add_css_class("settings-row-title");

    let chevron = gtk::Image::from_icon_name("go-next-symbolic");
    chevron.set_pixel_size(12);
    chevron.set_valign(gtk::Align::Center);
    chevron.add_css_class("settings-nav-chevron");

    row.append(&icon);
    row.append(&label);
    row.append(&chevron);
    row
}

/// A row with a title and a pair of labelled checkboxes.
fn checks_row(title: &str, checks: &[(&str, bool)]) -> gtk::Box {
    let boxes = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    for (label, active) in checks {
        let check = gtk::CheckButton::with_label(label);
        check.set_active(*active);
        boxes.append(&check);
    }
    settings_row(title, None, &boxes)
}

/// One column of the two-slider card: heading, slider, end labels.
fn slider_column(title: &str, scale: &gtk::Scale, low: &str, high: &str) -> gtk::Box {
    let heading = gtk::Label::new(Some(title));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("settings-row-title");
    let low = gtk::Label::new(Some(low));
    low.add_css_class("settings-slider-caption");
    low.set_halign(gtk::Align::Start);
    low.set_hexpand(true);
    let high = gtk::Label::new(Some(high));
    high.add_css_class("settings-slider-caption");
    high.set_halign(gtk::Align::End);
    let captions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    captions.append(&low);
    captions.append(&high);

    scale.set_draw_value(false);
    scale.set_hexpand(true);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_hexpand(true);
    column.append(&heading);
    column.append(scale);
    column.append(&captions);
    column
}

/// Name shown for a wallpaper: its file name, or "Default" for the bundled one.
fn wallpaper_name(path: Option<&Path>) -> String {
    path.and_then(|path| path.file_stem())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Default".to_string())
}

/// One clickable thumbnail. `None` is the bundled wallpaper.
fn wallpaper_tile(path: Option<&Path>) -> gtk::Button {
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_size_request(104, 66);
    match path {
        None => {
            if let Some(texture) = crate::wallpaper::small_texture(None, 320, 200) {
                picture.set_paintable(Some(&texture));
            }
        }
        Some(path) => {
            // Decode after the page is shown, one thumbnail per idle slot.
            let weak = picture.downgrade();
            let path = path.to_path_buf();
            glib::idle_add_local_once(move || {
                let Some(picture) = weak.upgrade() else { return };
                // Gallery thumbnails are ~104x66 px: decode them small instead of
                // keeping every full-size wallpaper in memory.
                if let Ok(pixbuf) =
                    gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&path, 320, 200, true)
                {
                    picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&pixbuf)));
                }
            });
        }
    }

    let button = gtk::Button::new();
    button.add_css_class("wallpaper-thumb");
    button.set_overflow(gtk::Overflow::Hidden);
    button.set_child(Some(&picture));
    button.set_tooltip_text(Some(&wallpaper_name(path)));
    let target = path.map(Path::to_path_buf);
    button.connect_clicked(move |_| crate::preferences::set_wallpaper_path(target.clone()));
    button
}

/// The Wallpaper page: a live preview, a gallery of wallpapers (the bundled
/// one plus any added) and an "Add Photo..." button. Changes apply at once.
fn build_wallpaper_page(body: &gtk::Box) {
    // ---- preview ----
    let preview = gtk::Picture::new();
    preview.set_content_fit(gtk::ContentFit::Cover);
    preview.set_can_shrink(true);
    preview.set_size_request(-1, 300);
    preview.set_hexpand(true);

    let preview_frame = gtk::Frame::new(None);
    preview_frame.add_css_class("settings-wallpaper-preview");
    preview_frame.set_overflow(gtk::Overflow::Hidden);
    preview_frame.set_child(Some(&preview));

    let caption = gtk::Label::new(None);
    caption.set_halign(gtk::Align::Start);
    caption.add_css_class("settings-wallpaper-caption");
    caption.set_margin_start(12);
    caption.set_margin_bottom(10);

    let preview_card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    preview_card.add_css_class("settings-group");
    preview_card.set_margin_top(4);
    preview_card.set_margin_bottom(8);
    preview_card.set_margin_start(8);
    preview_card.set_margin_end(8);
    preview_card.append(&preview_frame);
    preview_card.append(&caption);
    body.append(&preview_card);

    // ---- gallery ----
    body.append(&section_heading("Choose a Wallpaper"));
    let thumbs = gtk::FlowBox::new();
    thumbs.add_css_class("wallpaper-grid");
    thumbs.set_selection_mode(gtk::SelectionMode::None);
    thumbs.set_homogeneous(true);
    thumbs.set_min_children_per_line(4);
    thumbs.set_max_children_per_line(4);
    thumbs.set_row_spacing(8);
    thumbs.set_column_spacing(8);
    thumbs.set_margin_top(12);
    thumbs.set_margin_start(12);
    thumbs.set_margin_end(12);

    let add = gtk::Button::with_label("Add Photo\u{2026}");
    add.add_css_class("wallpaper-add");
    add.set_halign(gtk::Align::Start);
    add.set_margin_start(12);
    add.set_margin_bottom(12);

    let gallery_card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    gallery_card.add_css_class("settings-group");
    gallery_card.set_margin_start(8);
    gallery_card.set_margin_end(8);
    gallery_card.append(&thumbs);
    gallery_card.append(&add);
    body.append(&gallery_card);

    // ---- behaviour ----
    let tiles: Rc<RefCell<Vec<(Option<PathBuf>, gtk::Button)>>> = Rc::new(RefCell::new(Vec::new()));

    // Updates the preview, caption and highlighted thumbnail.
    let sync: Rc<dyn Fn()> = {
        let preview = preview.downgrade();
        let caption = caption.downgrade();
        let tiles = Rc::clone(&tiles);
        Rc::new(move || {
            let (Some(preview), Some(caption)) = (preview.upgrade(), caption.upgrade()) else {
                return;
            };
            let current = crate::preferences::wallpaper_path();
            // A small copy: the preview is only ~700x300 px.
            if let Some(texture) = crate::wallpaper::small_texture(current.as_deref(), 1440, 900) {
                preview.set_paintable(Some(&texture));
            }
            caption.set_text(&format!(
                "Current Wallpaper: {}",
                wallpaper_name(current.as_deref())
            ));
            for (path, button) in tiles.borrow().iter() {
                if *path == current {
                    button.add_css_class("wallpaper-thumb-selected");
                } else {
                    button.remove_css_class("wallpaper-thumb-selected");
                }
            }
        })
    };

    // Rebuilds the gallery from the library on disk.
    let rebuild: Rc<dyn Fn()> = {
        let thumbs = thumbs.downgrade();
        let tiles = Rc::clone(&tiles);
        let sync = Rc::clone(&sync);
        Rc::new(move || {
            let Some(thumbs) = thumbs.upgrade() else { return };
            while let Some(child) = thumbs.first_child() {
                thumbs.remove(&child);
            }
            let current = crate::preferences::wallpaper_path();
            let mut entries: Vec<Option<PathBuf>> = vec![None];
            entries.extend(crate::wallpaper::library().into_iter().map(Some));
            // A wallpaper chosen from outside the library still gets a tile.
            if current.is_some() && !entries.contains(&current) {
                entries.insert(1, current);
            }
            let mut tiles = tiles.borrow_mut();
            tiles.clear();
            for entry in entries {
                let button = wallpaper_tile(entry.as_deref());
                thumbs.append(&button);
                tiles.push((entry, button));
            }
            drop(tiles);
            sync();
        })
    };

    {
        let rebuild = Rc::clone(&rebuild);
        add.connect_clicked(move |button| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Choose Wallpaper")
                .modal(true)
                .filters(&filters)
                .default_filter(&filter)
                .build();
            let window = button.root().and_downcast::<gtk::Window>();
            let rebuild = Rc::clone(&rebuild);
            dialog.open(window.as_ref(), gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else { return };
                let Some(path) = file.path() else { return };
                match crate::wallpaper::import(&path) {
                    Ok(copy) => {
                        rebuild();
                        crate::preferences::set_wallpaper_path(Some(copy));
                    }
                    Err(error) => {
                        eprintln!("lunar-desktop: could not use {}: {error}", path.display())
                    }
                }
            });
        });
    }

    // Follow changes made elsewhere (e.g. the desktop context menu), and keep
    // the observer alive exactly as long as this page.
    crate::preferences::observe_wallpaper(&sync);
    preview_card.connect_destroy(move |_| {
        let _keep_alive = &sync;
    });
    rebuild();
}

/// General page: hero header plus a list of related settings links.
fn build_general_page(body: &gtk::Box, app: &gtk::Application) {
    // ---- Hero: gear icon, title, description (matches macOS System Settings) ----
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 8);
    hero.add_css_class("settings-page-hero");
    hero.set_halign(gtk::Align::Center);
    hero.set_margin_top(12);
    hero.set_margin_bottom(20);

    let icon = gtk::Image::from_icon_name("preferences-system-symbolic");
    icon.set_pixel_size(56);
    icon.add_css_class("settings-page-hero-icon");
    icon.set_halign(gtk::Align::Center);

    let title = gtk::Label::new(Some("General"));
    title.add_css_class("settings-page-hero-title");
    title.set_halign(gtk::Align::Center);

    let description = gtk::Label::new(Some(
        "Manage your overall setup and preferences for Mac, such as software updates, device language, AirDrop, and more.",
    ));
    description.add_css_class("settings-page-hero-description");
    description.set_halign(gtk::Align::Center);
    description.set_justify(gtk::Justification::Center);
    description.set_wrap(true);
    description.set_max_width_chars(42);
    description.set_xalign(0.5);

    hero.append(&icon);
    hero.append(&title);
    hero.append(&description);
    body.append(&hero);

    // ---- Linked pages ----
    let about = navigation_row("About", "dialog-information-symbolic");
    let about_app = app.clone();
    {
        // Make the whole row clickable by wrapping gesture on the box.
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            crate::about::present(&about_app);
        });
        about.add_controller(gesture);
    }

    let software_update = navigation_row("Software Update", "software-update-available-symbolic");
    {
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| open_subpage("software-update", "Software Update"));
        software_update.add_controller(gesture);
    }
    let storage = navigation_row("Storage", "drive-harddisk-symbolic");

    body.append(&settings_group(&[about, software_update, storage]));
}

/// Appearance page: Dark Mode and the Liquid Glass strength slider.
///
/// The slider drives the KWin glass plugin (see `glass_plugin`): the value is
/// saved at once, but KWin is only told once the slider has been still for a
/// moment, since each update reloads the compositor's effect settings.
fn build_appearance_page(body: &gtk::Box, app: &gtk::Application) {
    let prefs = crate::preferences::current();

    body.append(&section_heading("Appearance"));
    let dark_mode = gtk::Switch::new();
    dark_mode.set_active(prefs.dark_mode);
    let app = app.clone();
    dark_mode.connect_active_notify(move |switch| {
        crate::preferences::set_dark_mode(switch.is_active(), &app);
    });
    body.append(&settings_group(&[settings_row("Dark Mode", None, &dark_mode)]));

    body.append(&section_heading("Liquid Glass"));
    let strength = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    strength.set_value(prefs.liquid_glass_strength);
    let generation = std::rc::Rc::new(std::cell::Cell::new(0u32));
    strength.connect_value_changed(move |scale| {
        let value = scale.value();
        crate::preferences::set_liquid_glass_strength(value);
        let this = generation.get().wrapping_add(1);
        generation.set(this);
        let generation = std::rc::Rc::clone(&generation);
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            // A newer slider move supersedes this one.
            if generation.get() == this {
                crate::glass_plugin::apply_in_background(value);
            }
        });
    });
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    card.add_css_class("settings-row");
    card.append(&slider_column("Blur strength", &strength, "Light", "Strong"));
    body.append(&settings_group(&[card]));
}

// ---- sub-pages (pages opened from inside another page) ----------------

struct Nav {
    stack: glib::WeakRef<gtk::Stack>,
    title: glib::WeakRef<gtk::Label>,
    back: glib::WeakRef<gtk::Button>,
    forward: glib::WeakRef<gtk::Button>,
    /// Page to return to, and the sub-page that was opened from it.
    parent: Option<(String, String)>,
    sub: Option<(String, String)>,
}

thread_local! {
    static NAV: RefCell<Option<Nav>> = const { RefCell::new(None) };
}

fn nav_show(name: &str, title: &str, back: bool, forward: bool) {
    let parts = NAV.with(|nav| {
        nav.borrow().as_ref().map(|nav| {
            (nav.stack.upgrade(), nav.title.upgrade(), nav.back.upgrade(), nav.forward.upgrade())
        })
    });
    let Some((stack, label, back_button, forward_button)) = parts else {
        return;
    };
    if let Some(stack) = stack {
        stack.set_visible_child_name(name);
    }
    if let Some(label) = label {
        label.set_text(title);
    }
    if let Some(button) = back_button {
        button.set_sensitive(back);
    }
    if let Some(button) = forward_button {
        button.set_sensitive(forward);
    }
}

/// Opens a sub-page (see `build_subpage`) in the content area; the Back
/// button returns to the page it was opened from.
pub(crate) fn open_subpage(name: &str, title: &str) {
    let stack = NAV.with(|nav| nav.borrow().as_ref().and_then(|nav| nav.stack.upgrade()));
    let Some(stack) = stack else {
        return;
    };
    if stack.child_by_name(name).is_none() {
        stack.add_named(&build_subpage(name), Some(name));
    }
    let parent = stack.visible_child_name().map(|parent| {
        let parent_title = PAGES.iter().find(|page| page.name == parent).map_or("", |page| page.title);
        (parent.to_string(), parent_title.to_string())
    });
    NAV.with(|nav| {
        if let Some(nav) = nav.borrow_mut().as_mut() {
            nav.parent = parent;
            nav.sub = Some((name.to_string(), title.to_string()));
        }
    });
    nav_show(name, title, true, false);
}

fn nav_back() {
    let parent = NAV.with(|nav| nav.borrow().as_ref().and_then(|nav| nav.parent.clone()));
    if let Some((name, title)) = parent {
        nav_show(&name, &title, false, true);
    }
}

fn nav_forward() {
    let sub = NAV.with(|nav| nav.borrow().as_ref().and_then(|nav| nav.sub.clone()));
    if let Some((name, title)) = sub {
        nav_show(&name, &title, true, false);
    }
}

/// A sidebar page was chosen: forget any sub-page history.
fn nav_reset() {
    let buttons = NAV.with(|nav| {
        let mut nav = nav.borrow_mut();
        nav.as_mut().map(|nav| {
            nav.parent = None;
            nav.sub = None;
            (nav.back.upgrade(), nav.forward.upgrade())
        })
    });
    if let Some((back, forward)) = buttons {
        if let Some(back) = back {
            back.set_sensitive(false);
        }
        if let Some(forward) = forward {
            forward.set_sensitive(false);
        }
    }
}

fn build_subpage(name: &str) -> gtk::ScrolledWindow {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("settings-page");
    match name {
        "software-update" => crate::software_update_page::build(&body),
        _ => {}
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body));
    scroller
}

/// The scrolling body of one settings page.
fn build_page(name: &str, app: &gtk::Application) -> gtk::ScrolledWindow {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("settings-page");

    match name {
        "general" => build_general_page(&body, app),
        "desktop-dock" => {
            let prefs = crate::preferences::current();

            // ---- Dock ----
            body.append(&section_heading("Dock"));
            let size = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
            size.set_value(prefs.dock_size);
            size.connect_value_changed(|scale| {
                crate::preferences::set_dock_size(scale.value());
            });
            let magnification = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
            magnification.set_value(prefs.dock_magnification);
            magnification.connect_value_changed(|scale| {
                crate::preferences::set_dock_magnification(scale.value());
            });
            let sliders = gtk::Box::new(gtk::Orientation::Horizontal, 28);
            sliders.add_css_class("settings-row");
            sliders.append(&slider_column("Size", &size, "Small", "Large"));
            sliders.append(&slider_column("Magnification", &magnification, "Off", "Large"));

            let (minimize_into_icon, minimize_switch) = switch_row(
                "Minimize windows into application icon",
                None,
                prefs.minimize_into_icon,
            );
            minimize_switch.connect_active_notify(|switch| {
                crate::preferences::set_minimize_into_icon(switch.is_active());
            });
            let (animate, _) = switch_row("Animate opening applications", None, true);
            let (autohide_row, autohide) = switch_row("Automatically hide and show the Dock", None, prefs.dock_auto_hide);
            autohide.connect_active_notify(|switch| {
                crate::preferences::set_dock_auto_hide(switch.is_active());
            });
            let (indicators, _) = switch_row("Show indicators for open applications", None, true);
            let (suggested, _) = switch_row("Show suggested and recent apps in Dock", None, true);
            body.append(&settings_group(&[
                sliders,
                popup_row("Position on screen", &["Left", "Bottom", "Right"], 1),
                popup_row("Minimize windows using", &["Genie effect", "Scale effect"], 0),
                popup_row("Double-click a window\u{2019}s title bar to", &["Zoom", "Minimize", "Do Nothing"], 0),
                minimize_into_icon,
                animate,
                autohide_row,
                indicators,
                suggested,
            ]));

            // ---- Desktop & Stage Manager ----
            body.append(&section_heading("Desktop & Stage Manager"));
            let (stage_manager, _) = switch_row(
                "Stage Manager",
                Some("Stage Manager arranges your recent windows into a single strip for reduced clutter and quick access."),
                false,
            );
            body.append(&settings_group(&[
                checks_row("Show Items", &[("On Desktop", true), ("In Stage Manager", false)]),
                popup_row("Click wallpaper to reveal Desktop", &["Always", "Only in Stage Manager"], 0),
                stage_manager,
            ]));

            // ---- Widgets ----
            body.append(&section_heading("Widgets"));
            let (iphone_widgets, _) = switch_row("Use iPhone widgets", None, true);
            body.append(&settings_group(&[
                checks_row("Show Widgets", &[("On Desktop", true), ("In Stage Manager", true)]),
                popup_row("Widget style", &["Automatic", "Monochrome", "Full-color"], 0),
                iphone_widgets,
            ]));

        }
        "appearance" => build_appearance_page(&body, app),
        "wifi" => crate::wifi_page::build(&body),
        "displays" => crate::displays_page::build(&body),
        "bluetooth" => crate::bluetooth_page::build(&body),
        "wallpaper" => build_wallpaper_page(&body),
        _ => {
            let autohide = gtk::Switch::new();
            body.append(&settings_group(&[settings_row(
                "Automatically hide and show the menu bar",
                None,
                &autohide,
            )]));
        }
    }

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body));
    scroller
}

/// The pill-shaped back/forward control. Not wired up yet, so both are disabled.
fn navigation_pill() -> (gtk::Box, gtk::Button, gtk::Button) {
    let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    navigation.add_css_class("settings-navigation");
    navigation.set_valign(gtk::Align::Center);
    let mut buttons = Vec::new();
    for (index, (icon_name, tooltip)) in [
        ("go-previous-symbolic", "Back"),
        ("go-next-symbolic", "Forward"),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            let divider = gtk::Separator::new(gtk::Orientation::Vertical);
            divider.add_css_class("settings-navigation-divider");
            navigation.append(&divider);
        }
        let button = gtk::Button::new();
        button.add_css_class("settings-navigation-button");
        button.set_tooltip_text(Some(tooltip));
        button.set_sensitive(false);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        button.set_child(Some(&icon));
        navigation.append(&button);
        buttons.push(button);
    }
    let forward = buttons.pop().unwrap();
    let back = buttons.pop().unwrap();
    (navigation, back, forward)
}

fn compact_window_control_children(widget: &gtk::Widget) {
    let mut child = widget.first_child();
    while let Some(control) = child {
        child = control.next_sibling();
        if let Ok(button) = control.clone().downcast::<gtk::Button>() {
            button.add_css_class("settings-window-control");
            button.set_size_request(14, 14);
            button.set_margin_top(0);
            button.set_margin_bottom(0);
            button.set_margin_start(0);
            button.set_margin_end(4);
        } else if let Ok(image) = control.clone().downcast::<gtk::Image>() {
            // Hidden via opacity alone, the symbol still contributes its
            // natural size to the button's minimum measurement.
            image.set_visible(false);
        }
        compact_window_control_children(&control);
    }
}

/// The settings window, if it is open.
pub fn find_window(app: &gtk::Application) -> Option<gtk::Window> {
    app.windows()
        .into_iter()
        .find(|window| window.title().as_deref() == Some(WINDOW_TITLE))
}

/// Opens the settings window on the page called `name` (see `PAGES`).
pub fn show_page(app: &gtk::Application, name: &str) {
    present(app);
    let Some(index) = PAGES.iter().position(|page| page.name == name) else {
        return;
    };
    let list = SIDEBAR.with(|sidebar| sidebar.borrow().as_ref().and_then(|weak| weak.upgrade()));
    if let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) {
        if let Some(list) = row.parent().and_downcast::<gtk::ListBox>() {
            list.select_row(Some(&row));
        }
    }
}

/// Presents the settings window, creating it on first use.
pub fn present(app: &gtk::Application) {
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|window| window.title().as_deref() == Some(WINDOW_TITLE))
    {
        window.present();
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .default_width(WINDOW_WIDTH)
        .default_height(640)
        .resizable(true)
        .css_classes(["lunar-settings-window"])
        .build();
    // An empty title bar makes GTK draw the window frame itself instead of
    // KWin, so the sidebar can reach the top edge. The traffic-light buttons
    // live in the sidebar (below).
    let titlebar_stub = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    titlebar_stub.add_css_class("settings-titlebar-stub");
    window.set_titlebar(Some(&titlebar_stub));

    // ---- sidebar -------------------------------------------------------
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("settings-sidebar");
    sidebar.set_size_request(SIDEBAR_WIDTH, -1);
    sidebar.set_hexpand(false);

    let controls = gtk::WindowControls::new(gtk::PackType::Start);
    controls.set_decoration_layout(Some("close,minimize,maximize:"));
    controls.add_css_class("settings-window-controls");
    controls.set_halign(gtk::Align::Start);
    controls.set_valign(gtk::Align::Center);
    compact_window_control_children(controls.upcast_ref());
    // Dragging the empty space around the buttons moves the window.
    let sidebar_top = gtk::WindowHandle::new();
    sidebar_top.add_css_class("settings-sidebar-top");
    sidebar_top.set_child(Some(&controls));
    let controls_on_map = controls.downgrade();
    sidebar_top.connect_map(move |_| {
        if let Some(controls) = controls_on_map.upgrade() {
            compact_window_control_children(controls.upcast_ref());
        }
    });
    sidebar.append(&sidebar_top);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search"));
    search.add_css_class("settings-search");
    sidebar.append(&search);

    sidebar.append(&account_block());

    let list = gtk::ListBox::new();
    list.add_css_class("settings-sidebar-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    // A small gap above the first row of every new group.
    list.set_header_func(|row, before| {
        let group_of = |row: &gtk::ListBoxRow| PAGES.get(row.index() as usize).map(|page| page.group);
        let starts_new_group = before.is_some_and(|before| group_of(before) != group_of(row));
        if starts_new_group {
            let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
            gap.add_css_class("settings-sidebar-gap");
            row.set_header(Some(&gap));
        } else {
            row.set_header(None::<&gtk::Widget>);
        }
    });
    for page in &PAGES {
        list.append(&sidebar_row(page));
    }
    SIDEBAR.with(|sidebar| *sidebar.borrow_mut() = Some(list.downgrade()));
    let list_scroller = gtk::ScrolledWindow::new();
    list_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    list_scroller.set_vexpand(true);
    list_scroller.set_child(Some(&list));
    sidebar.append(&list_scroller);

    // ---- content -------------------------------------------------------
    let title = gtk::Label::new(Some(PAGES[0].title));
    title.set_halign(gtk::Align::Start);
    title.add_css_class("settings-page-title");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    toolbar.add_css_class("settings-toolbar");
    let (pill, back_button, forward_button) = navigation_pill();
    back_button.connect_clicked(|_| nav_back());
    forward_button.connect_clicked(|_| nav_forward());
    toolbar.append(&pill);
    toolbar.append(&title);
    let toolbar_handle = gtk::WindowHandle::new();
    toolbar_handle.set_child(Some(&toolbar));

    // Pages are built the first time they are shown: the Wallpaper page in
    // particular decodes images, which is wasted work (and memory) if the
    // user never opens it.
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::None);
    let app_for_rows = app.clone();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("settings-content");
    content.set_hexpand(true);
    content.append(&toolbar_handle);
    content.append(&stack);

    // Selecting a sidebar row shows its page and updates the heading.
    let stack_for_rows = stack.downgrade();
    let title_for_rows = title.downgrade();
    list.connect_row_selected(move |_, row| {
        let Some(page) = row.and_then(|row| PAGES.get(row.index() as usize)) else {
            return;
        };
        if let Some(stack) = stack_for_rows.upgrade() {
            if stack.child_by_name(page.name).is_none() {
                stack.add_named(&build_page(page.name, &app_for_rows), Some(page.name));
            }
            stack.set_visible_child_name(page.name);
        }
        if let Some(title) = title_for_rows.upgrade() {
            title.set_text(page.title);
        }
        nav_reset();
    });
    // Clicking the page that is already selected leaves any sub-page.
    let stack_for_activate = stack.downgrade();
    let title_for_activate = title.downgrade();
    list.connect_row_activated(move |_, row| {
        let Some(page) = PAGES.get(row.index() as usize) else {
            return;
        };
        if let Some(stack) = stack_for_activate.upgrade() {
            stack.set_visible_child_name(page.name);
        }
        if let Some(title) = title_for_activate.upgrade() {
            title.set_text(page.title);
        }
        nav_reset();
    });
    NAV.with(|nav| {
        *nav.borrow_mut() = Some(Nav {
            stack: stack.downgrade(),
            title: title.downgrade(),
            back: back_button.downgrade(),
            forward: forward_button.downgrade(),
            parent: None,
            sub: None,
        });
    });
    let first_page = PAGES.iter().position(|page| page.name == "general").unwrap_or(0);
    if let Some(first) = list.row_at_index(first_page as i32) {
        list.select_row(Some(&first));
    }

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.add_css_class("settings-root");
    // Clip the children to the rounded corners (CSS cannot do this in GTK).
    layout.set_overflow(gtk::Overflow::Hidden);
    layout.append(&sidebar);
    layout.append(&content);
    window.set_child(Some(&layout));

    // Fixed width, adjustable height. GTK has no maximum-size API, so the
    // limits go straight to the compositor (Wayland only); the minimum width
    // below also stops the window getting narrower on any backend.
    window.set_size_request(WINDOW_WIDTH, WINDOW_MIN_HEIGHT);
    window.connect_realize(lock_window_width);
    window.connect_map(|window| {
        lock_window_width(window);
        // GTK may send its own limits with the first frame; send ours again.
        let window = window.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
            if let Some(window) = window.upgrade() {
                lock_window_width(&window);
            }
        });
    });

    // After the window is gone, give its freed memory back to the OS.
    window.connect_destroy(|_| {
        glib::idle_add_local_once(crate::thumbnail::release_freed_memory);
    });

    crate::preferences::apply_dark_mode(&window);
    crate::context_menu::attach_window_blur(&window);
    window.present();
}
