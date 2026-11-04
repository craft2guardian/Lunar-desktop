//! Persisted preferences shared by the settings window and live dock surfaces.

use std::cell::RefCell;
use std::path::PathBuf;

use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preferences {
    /// 0–100, with 50 preserving the dock's original monitor-scaled size.
    pub dock_size: f64,
    pub dock_auto_hide: bool,
    /// 0–100: how much icons grow under the pointer (0 = off, 100 = double).
    pub dock_magnification: f64,
    pub dark_mode: bool,
    /// On: a minimised window disappears into its application's Dock icon.
    /// Off: it gets its own thumbnail tile on the right of the Dock.
    pub minimize_into_icon: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            dock_size: 50.0,
            dock_auto_hide: false,
            dock_magnification: 60.0,
            dark_mode: false,
            minimize_into_icon: true,
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Preferences> = RefCell::new(load());
    static SIZE_OBSERVERS: RefCell<Vec<std::rc::Weak<dyn Fn(f64)>>> = RefCell::new(Vec::new());
    /// The wallpaper image chosen in Settings; `None` means the bundled one.
    static WALLPAPER: RefCell<Option<PathBuf>> = RefCell::new(load_wallpaper());
    static WALLPAPER_OBSERVERS: RefCell<Vec<std::rc::Weak<dyn Fn()>>> = RefCell::new(Vec::new());
}

pub fn wallpaper_path() -> Option<PathBuf> {
    WALLPAPER.with(|wallpaper| wallpaper.borrow().clone())
}

/// Chooses the wallpaper (`None` = bundled), saves it and tells every observer.
pub fn set_wallpaper_path(path: Option<PathBuf>) {
    let changed = WALLPAPER.with(|wallpaper| {
        let mut wallpaper = wallpaper.borrow_mut();
        if *wallpaper == path {
            false
        } else {
            *wallpaper = path;
            true
        }
    });
    if !changed {
        return;
    }
    persist_current();
    let observers: Vec<std::rc::Rc<dyn Fn()>> = WALLPAPER_OBSERVERS.with(|observers| {
        let mut observers = observers.borrow_mut();
        observers.retain(|observer| observer.strong_count() > 0);
        observers.iter().filter_map(|observer| observer.upgrade()).collect()
    });
    for observer in observers {
        observer();
    }
}

/// Calls `observer` whenever the wallpaper choice changes, for as long as the
/// caller keeps the `Rc` alive.
pub fn observe_wallpaper(observer: &std::rc::Rc<dyn Fn()>) {
    WALLPAPER_OBSERVERS.with(|observers| {
        observers
            .borrow_mut()
            .push(std::rc::Rc::downgrade(observer))
    });
}

pub fn current() -> Preferences {
    CURRENT.with(|preferences| *preferences.borrow())
}

pub fn set_dock_size(value: f64) {
    set_dock_size_live(value);
    persist_current();
}

/// Updates the in-memory value during a drag; call `persist_current` on release.
pub fn set_dock_size_live(value: f64) {
    let value = value.clamp(0.0, 100.0);
    let changed = CURRENT.with(|preferences| {
        let mut preferences = preferences.borrow_mut();
        if (preferences.dock_size - value).abs() < f64::EPSILON {
            false
        } else {
            preferences.dock_size = value;
            true
        }
    });
    if changed {
        let observers = SIZE_OBSERVERS.with(|observers| {
            let mut observers = observers.borrow_mut();
            let mut live = Vec::new();
            observers.retain(|observer| {
                if let Some(observer) = observer.upgrade() {
                    live.push(observer);
                    true
                } else {
                    false
                }
            });
            live
        });
        for observer in observers {
            observer(value);
        }
    }
}

pub fn observe_dock_size(observer: &std::rc::Rc<dyn Fn(f64)>) {
    SIZE_OBSERVERS.with(|observers| {
        observers
            .borrow_mut()
            .push(std::rc::Rc::downgrade(observer))
    });
}

pub fn persist_current() {
    if let Err(error) = save(current()) {
        eprintln!("lunar-desktop: could not save settings: {error}");
    }
}

pub fn set_dock_auto_hide(value: bool) {
    update(|preferences| preferences.dock_auto_hide = value);
}

/// Sets the hover magnification (0–100, 0 = off).
pub fn set_dock_magnification(value: f64) {
    update(|preferences| preferences.dock_magnification = value.clamp(0.0, 100.0));
}

pub fn set_minimize_into_icon(value: bool) {
    update(|preferences| preferences.minimize_into_icon = value);
}

pub fn set_dark_mode(value: bool, app: &gtk::Application) {
    update(|preferences| preferences.dark_mode = value);
    apply_dark_mode_to_open_windows(app);
}

/// Apply the saved appearance to a newly created window and its widget tree.
pub fn apply_dark_mode(window: &impl IsA<gtk::Widget>) {
    set_dark_mode_class(window.as_ref(), current().dark_mode);
}

fn apply_dark_mode_to_open_windows(app: &gtk::Application) {
    let dark = current().dark_mode;
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    for window in app.windows() {
        set_dark_mode_class(window.upcast_ref(), dark);
    }
}

fn set_dark_mode_class(widget: &gtk::Widget, dark: bool) {
    let currently_dark = widget.has_css_class("dark-mode");
    if currently_dark != dark {
        if dark {
            widget.add_css_class("dark-mode");
        } else {
            widget.remove_css_class("dark-mode");
        }
        // GlassStrip paints its tint in snapshot(), so explicitly request a
        // new frame when the appearance class changes.
        widget.queue_draw();
    }
    if widget.has_css_class("dock-trash-icon") {
        widget.set_opacity(if dark { 0.78 } else { 1.0 });
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        set_dark_mode_class(&current, dark);
    }
}

fn update(change: impl FnOnce(&mut Preferences)) {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        change(&mut current);
        if let Err(error) = save(*current) {
            eprintln!("lunar-desktop: could not save settings: {error}");
        }
    });
}

/// `~/.config/lunar-desktop` (or under `$XDG_CONFIG_HOME`).
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|directory| directory.join("lunar-desktop"))
}

/// One-time move of the settings folder from its old name (`sonoma-desktop`)
/// to `lunar-desktop`, so the chosen wallpaper and preferences survive the
/// rename. Does nothing if the new folder already exists.
pub fn migrate_old_config() {
    let Some(new_dir) = config_dir() else { return };
    let Some(parent) = new_dir.parent() else { return };
    let old_dir = parent.join(concat!("sonoma", "-desktop"));
    if new_dir.exists() || !old_dir.is_dir() {
        return;
    }
    if std::fs::rename(&old_dir, &new_dir).is_err() {
        return;
    }
    // settings.toml keeps the wallpaper as an absolute path into the old folder.
    let settings = new_dir.join("settings.toml");
    if let Ok(text) = std::fs::read_to_string(&settings) {
        let fixed = text.replace(
            concat!("/sonoma", "-desktop/"),
            "/lunar-desktop/",
        );
        if fixed != text {
            let _ = std::fs::write(&settings, fixed);
        }
    }
}

fn path() -> Option<PathBuf> {
    config_dir().map(|directory| directory.join("settings.toml"))
}

fn load_wallpaper() -> Option<PathBuf> {
    let text = std::fs::read_to_string(path()?).ok()?;
    let table = text.parse::<toml::Table>().ok()?;
    let value = table.get("wallpaper")?.as_str()?;
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn load() -> Preferences {
    let Some(path) = path() else {
        return Preferences::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Preferences::default();
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        eprintln!("lunar-desktop: invalid settings file; using defaults");
        return Preferences::default();
    };
    Preferences {
        dock_size: table
            .get("dock_size")
            .and_then(|value| {
                value
                    .as_float()
                    .or_else(|| value.as_integer().map(|integer| integer as f64))
            })
            .unwrap_or(50.0)
            .clamp(0.0, 100.0),
        dock_auto_hide: table
            .get("dock_auto_hide")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        dock_magnification: table
            .get("dock_magnification")
            .and_then(|value| {
                value
                    .as_float()
                    .or_else(|| value.as_integer().map(|integer| integer as f64))
            })
            .unwrap_or(60.0)
            .clamp(0.0, 100.0),
        dark_mode: table
            .get("dark_mode")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        minimize_into_icon: table
            .get("minimize_into_icon")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
    }
}

fn save(preferences: Preferences) -> Result<(), String> {
    let path = path().ok_or("could not determine the configuration directory")?;
    let parent = path.parent().ok_or("invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut contents = format!(
        "dock_size = {}\ndock_auto_hide = {}\ndock_magnification = {}\ndark_mode = {}\nminimize_into_icon = {}\n",
        preferences.dock_size,
        preferences.dock_auto_hide,
        preferences.dock_magnification,
        preferences.dark_mode,
        preferences.minimize_into_icon
    );
    if let Some(wallpaper) = wallpaper_path() {
        // A TOML string, so quotes and backslashes in the path are escaped.
        let value = toml::Value::String(wallpaper.to_string_lossy().into_owned());
        contents.push_str(&format!("wallpaper = {value}\n"));
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}
