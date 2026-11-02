//! The Dock: a small centred `top`-layer surface at the bottom of each output.
//! Its tiles come from `config/dock.toml` (see `config.rs`). Clicking a tile
//! launches its command. Running apps get a small indicator beneath the icon.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use crate::context_menu::ContextMenu;
use crate::kde_shell::{self, Role};
use crate::kwin_windows::{self, KWinWindows, WindowSnapshot};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{self, Entry};

const ICON_CORNER_RADIUS_FACTOR: f32 = 0.22;

/// Launch bounce: seconds per hop, and the tallest hop in px. The Dock window
/// has 24px of transparent margin above the shelf (see `.dock` in style.rs), so
/// the icon can rise this far without being clipped.
const BOUNCE_PERIOD_SECONDS: f64 = 0.75;
const BOUNCE_MAX_LIFT: f32 = 11.3;

/// How long a tile takes to slide into the Dock (matches the Revealer's
/// transition duration set in `create_tile`). The bounce starts right after.
const EXPAND_MILLIS: u64 = 180;

mod rounded_icon_imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct RoundedIcon {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) size: std::cell::Cell<i32>,
        /// Current launch-bounce height in px (0 when at rest).
        pub(super) lift: std::cell::Cell<f32>,
        pub(super) bouncing: std::cell::Cell<bool>,
        pub(super) stop_bounce: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoundedIcon {
        const NAME: &'static str = "SonomaRoundedDockIcon";
        type Type = super::RoundedIcon;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for RoundedIcon {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for RoundedIcon {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.size.get().max(0);
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let child = self.child.borrow();
            let Some(child) = child.as_ref() else {
                return;
            };
            let widget = self.obj();
            let (width, height) = (widget.width(), widget.height());
            if width <= 0 || height <= 0 {
                return;
            }
            let lift = self.lift.get();
            if lift != 0.0 {
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(0.0, -lift));
            }
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let radius = width.min(height) as f32 * ICON_CORNER_RADIUS_FACTOR;
            let clip = gtk::gsk::RoundedRect::from_rect(bounds, radius);
            snapshot.push_rounded_clip(&clip);
            widget.snapshot_child(child, snapshot);
            snapshot.pop();
            if lift != 0.0 {
                snapshot.restore();
            }
        }
    }
}

glib::wrapper! {
    pub struct RoundedIcon(ObjectSubclass<rounded_icon_imp::RoundedIcon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl RoundedIcon {
    fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let icon: Self = glib::Object::new();
        child.set_parent(&icon);
        icon.imp().child.replace(Some(child.clone().upcast()));
        icon
    }

    fn set_icon_size(&self, size: i32) {
        self.imp().size.set(size.max(0));
        self.queue_resize();
    }

    /// Hops the icon up and down like a macOS Dock icon while its app opens.
    /// Keeps hopping until `stop_bounce` is called or `max` has elapsed, and
    /// always finishes the hop in progress so the icon lands at rest.
    /// `on_done` runs once it has landed.
    fn start_bounce(&self, max: Duration, on_done: impl Fn() + 'static) {
        let imp = self.imp();
        imp.stop_bounce.set(false);
        if imp.bouncing.replace(true) {
            return;
        }
        let began = std::cell::Cell::new(None::<i64>);
        let last_phase = std::cell::Cell::new(0.0_f64);
        self.add_tick_callback(move |icon, clock| {
            let imp = icon.imp();
            let now = clock.frame_time();
            let start = match began.get() {
                Some(start) => start,
                None => {
                    began.set(Some(now));
                    now
                }
            };
            let elapsed = (now - start) as f64 / 1_000_000.0;
            let phase = (elapsed / BOUNCE_PERIOD_SECONDS).fract();
            let wrapped = phase < last_phase.replace(phase);
            if wrapped && (imp.stop_bounce.get() || elapsed >= max.as_secs_f64()) {
                imp.lift.set(0.0);
                imp.bouncing.set(false);
                icon.queue_draw();
                on_done();
                return glib::ControlFlow::Break;
            }
            // A parabola: quick off the ground, slow at the top, like gravity.
            let peak = (icon.height() as f32 * 0.5).min(BOUNCE_MAX_LIFT);
            let p = phase as f32;
            imp.lift.set(peak * 4.0 * p * (1.0 - p));
            icon.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    /// Lets the current hop finish, then stops bouncing.
    fn stop_bounce(&self) {
        self.imp().stop_bounce.set(true);
    }
}

/// Icon tile size in logical px, scaled with the output height (clamped).
fn tile_size(monitor: &gdk::Monitor, preference: f64) -> i32 {
    let base = (monitor.geometry().height() as f64 * 0.058).clamp(46.0, 80.0);
    (base * (0.65 + preference / 100.0 * 0.7)).round() as i32
}

/// Spawns `program` fully detached from the Dock: in its own session (so it
/// is not part of the Dock's process group, terminal or job and survives the
/// Dock exiting or being killed), with stdin/stdout/stderr closed, and reaped
/// by a helper thread so no zombie is left behind.
pub(crate) fn spawn_detached(program: &std::ffi::OsStr, args: &[&std::ffi::OsStr]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    extern "C" {
        fn setsid() -> i32;
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and the closure does nothing else.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Runs `command` without a shell: it is split into program + arguments
/// (quotes group words, nothing is expanded or interpreted) and spawned
/// detached from the Dock (see `spawn_detached`). Failures are reported on
/// stderr and never affect the shell.
fn launch(command: &str) {
    let argv = match glib::shell_parse_argv(command) {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => return,
        Err(e) => {
            eprintln!("sonoma-desktop: bad dock command {command:?}: {e}");
            return;
        }
    };
    let args: Vec<&std::ffi::OsStr> = argv[1..].iter().map(|a| a.as_os_str()).collect();
    if let Err(e) = spawn_detached(argv[0].as_os_str(), &args) {
        eprintln!("sonoma-desktop: could not launch {command:?}: {e}");
    }
}

/// Finds the `.desktop` file of an installed app in the XDG data directories.
pub(crate) fn desktop_file_for(app: &gio::AppInfo) -> Option<std::path::PathBuf> {
    let id = app.id()?;
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    dirs.into_iter()
        .map(|dir| dir.join("applications").join(id.as_str()))
        .find(|path| path.is_file())
}

/// Launches an installed desktop entry through `gio launch` in a detached
/// process. Returns false if that was not possible.
fn launch_app_detached(app: &gio::AppInfo) -> bool {
    let Some(path) = desktop_file_for(app) else {
        return false;
    };
    spawn_detached(
        std::ffi::OsStr::new("gio"),
        &[std::ffi::OsStr::new("launch"), path.as_os_str()],
    )
    .is_ok()
}

fn installed_app_for(program: &str) -> Option<gio::AppInfo> {
    kwin_windows::installed_app_for_program(program)
        .or_else(|| kwin_windows::installed_app_for_identity(program))
}

/// Prefer the installed desktop entry for simple commands. This honors KDE's
/// DBus activation and startup metadata (notably Firefox's packaged launcher).
/// The shell's own application, for presenting the System Settings window.
fn shell_application() -> Option<gtk::Application> {
    gio::Application::default().and_then(|app| app.downcast::<gtk::Application>().ok())
}

/// System Settings runs inside this process, so its Dock tile acts on the
/// window directly instead of going through KWin or a launcher.
fn settings_window_action(action: &str) {
    let Some(app) = shell_application() else {
        return;
    };
    match action {
        "activate" => crate::settings::present(&app),
        "hide" => {
            if let Some(window) = crate::settings::find_window(&app) {
                window.minimize();
            }
        }
        "quit" => {
            if let Some(window) = crate::settings::find_window(&app) {
                window.close();
            }
        }
        _ => {}
    }
}

fn launch_entry(entry: &Entry) {
    if entry.command == kwin_windows::SETTINGS_IDENTITY {
        settings_window_action("activate");
        return;
    }
    if let Ok(argv) = glib::shell_parse_argv(&entry.command) {
        if argv.len() == 1 {
            let program = Path::new(argv[0].as_os_str())
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            if let Some(app) = program.as_deref().and_then(installed_app_for) {
                if launch_app_detached(&app) {
                    return;
                }
                if let Err(error) = app.launch(&[], None::<&gio::AppLaunchContext>) {
                    eprintln!(
                        "sonoma-desktop: desktop entry for {} failed to launch: {error}",
                        entry.name
                    );
                }
                return;
            }
        }
    }
    launch(&entry.command);
}

/// The program name of a command, e.g. "firefox" for "/usr/bin/firefox --new-window".
fn program_name(command: &str) -> Option<String> {
    let argv = glib::shell_parse_argv(command).ok()?;
    let first = argv.first()?;
    Some(Path::new(first).file_name()?.to_string_lossy().into_owned())
}

#[derive(Clone, Copy)]
enum DesktopKind {
    Hyprland,
    Kde,
    Other,
}

fn detect_desktop() -> DesktopKind {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return DesktopKind::Hyprland;
    }

    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if desktop
        .split([':', ';'])
        .any(|name| name.contains("kde") || name.contains("plasma"))
        || std::env::var_os("KDE_SESSION_VERSION").is_some()
    {
        DesktopKind::Kde
    } else {
        DesktopKind::Other
    }
}

/// Returns the executable or window-class names that currently have a Dock app open.
fn running_apps(desktop: DesktopKind) -> HashSet<String> {
    match desktop {
        DesktopKind::Hyprland => hyprland_window_classes().unwrap_or_else(running_process_names),
        // The persistent KWin tracker is the normal KDE source. Retain this
        // as a degraded fallback only when that bridge could not start.
        DesktopKind::Kde => running_process_names(),
        DesktopKind::Other => HashSet::new(),
    }
}

/// Read Hyprland's current client list so indicators represent actual windows.
fn hyprland_window_classes() -> Option<HashSet<String>> {
    let output = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let clients: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).ok()?;
    Some(
        clients
            .iter()
            .flat_map(|client| {
                ["class", "initialClass"]
                    .into_iter()
                    .filter_map(move |field| {
                        client
                            .get(field)
                            .and_then(serde_json::Value::as_str)
                            .filter(|value| !value.is_empty())
                            .map(str::to_ascii_lowercase)
                    })
            })
            .collect(),
    )
}

/// Process names are used only for launch de-duplication and Dock indicators.
/// Actual KDE window discovery and control goes through KWin's script API.
fn running_process_names() -> HashSet<String> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .parse::<u32>()
                .ok()
                .map(|_| entry)
        })
        .filter_map(|entry| std::fs::read_link(entry.path().join("exe")).ok())
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
        })
        .collect()
}

fn app_is_running(program: &str, running: &HashSet<String>) -> bool {
    let program = program.to_ascii_lowercase();
    running.iter().any(|name| {
        name == &program
            || name.strip_suffix(".desktop") == Some(program.as_str())
            || name.ends_with(&format!(".{program}"))
    })
}

/// Lists X11/XWayland windows managed by KWin that belong to this app.
fn x11_app_windows(program: &str) -> Vec<String> {
    let Ok(output) = Command::new("wmctrl").args(["-lpGx"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let program = program.to_ascii_lowercase();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let (id, pid, class) = (fields.first()?, fields.get(2)?, fields.get(7)?);
            let class = class.to_ascii_lowercase();
            let process_matches = pid.parse::<u32>().ok().is_some_and(|pid| {
                std::fs::read_link(format!("/proc/{pid}/exe"))
                    .ok()
                    .and_then(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_ascii_lowercase())
                    })
                    .is_some_and(|name| name == program)
            });
            let class_matches = class.split('.').any(|part| part == program)
                || class == program
                || class.ends_with(&format!(".{program}"));
            (process_matches || class_matches).then(|| (*id).to_string())
        })
        .collect()
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

static KWIN_ACTION_ID: AtomicU64 = AtomicU64::new(1);

fn kwin_window_action(program: &str, action: &str) -> Result<(), String> {
    let program =
        serde_json::to_string(&program.to_ascii_lowercase()).map_err(|error| error.to_string())?;
    let action = serde_json::to_string(action).map_err(|error| error.to_string())?;
    let script = format!(
        r#"const wanted = {program};
const action = {action};
const windows = workspace.stackingOrder
    || (typeof workspace.windowList === "function"
        ? workspace.windowList()
        : (typeof workspace.clientList === "function" ? workspace.clientList() : []));
const matches = windows.filter(function (window) {{
    const keys = [window.resourceClass, window.resourceName, window.desktopFileName]
        .map(function (value) {{ return String(value || "").toLowerCase(); }});
    return keys.some(function (value) {{
        const normalized = value.endsWith(".desktop") ? value.slice(0, -8) : value;
        return normalized === wanted || normalized.endsWith("." + wanted);
    }});
}});
if (action === "activate") {{
    const window = matches.find(function (candidate) {{ return candidate.active; }})
        || matches.find(function (candidate) {{ return !candidate.minimized; }})
        || matches[0];
    if (window && (!window.active || window.minimized)) {{
        window.minimized = false;
        workspace.activeWindow = window;
    }}
}} else if (action === "hide") {{
    matches.forEach(function (window) {{ window.minimized = true; }});
}} else if (action === "quit") {{
    matches.forEach(function (window) {{ if (window.closeable) window.closeWindow(); }});
}}
"#
    );
    run_kwin_script(&script)
}

/// Loads `script` into KWin, runs it once and unloads it again.
pub(crate) fn run_kwin_script(script: &str) -> Result<(), String> {
    let dbus = ["qdbus6", "qdbus"]
        .into_iter()
        .find(|name| Command::new(name).arg("--help").output().is_ok())
        .ok_or("qdbus is not installed")?;
    let serial = KWIN_ACTION_ID.fetch_add(1, Ordering::Relaxed);
    let plugin = format!("sonoma_dock_action_{}_{}", std::process::id(), serial);
    let path = std::env::temp_dir().join(format!("{plugin}.js"));
    std::fs::write(&path, script).map_err(|error| error.to_string())?;

    let loaded = match Command::new(dbus)
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
        ])
        .arg(&path)
        .arg(&plugin)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(error.to_string());
        }
    };
    if !loaded.status.success() {
        let _ = std::fs::remove_file(&path);
        return Err(kwin_windows::command_failure(&loaded));
    }
    let id = match String::from_utf8_lossy(&loaded.stdout)
        .trim()
        .parse::<i32>()
    {
        Ok(id) => id,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "KWin returned an invalid script id: {}",
                String::from_utf8_lossy(&loaded.stdout).trim()
            ));
        }
    };
    if id < 0 {
        let _ = std::fs::remove_file(&path);
        return Err("KWin rejected the window-management script".to_string());
    }

    let script_path = format!("/Scripting/Script{id}");
    // KWin loads the JavaScript asynchronously while handling run(), so keep
    // the temporary source available until the D-Bus call has completed.
    let run = Command::new(dbus)
        .args(["org.kde.KWin", &script_path, "org.kde.kwin.Script.run"])
        .output();
    let _ = std::fs::remove_file(&path);
    let stop = Command::new(dbus)
        .args(["org.kde.KWin", &script_path, "org.kde.kwin.Script.stop"])
        .output();
    let unload = Command::new(dbus)
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
            &plugin,
        ])
        .output();
    let run_error = match run {
        Ok(result) if result.status.success() => None,
        Ok(result) => Some(kwin_windows::command_failure(&result)),
        Err(error) => Some(error.to_string()),
    };
    if let Some(error) = run_error {
        let _ = stop;
        let _ = unload;
        return Err(error);
    }
    if unload.is_err() {
        eprintln!("sonoma-desktop: could not unload temporary KWin script {plugin}");
    }
    Ok(())
}

/// KWin script behind the menu bar's Window menu. `__ID__` is the target
/// window's id (or null for the active window), `__ACTION__` the action name.
const ACTIVE_WINDOW_SCRIPT: &str = r#"const wantedId = __ID__;
const action = __ACTION__;
const all = (typeof workspace.windowList === "function" ? workspace.windowList() : null)
    || workspace.stackingOrder
    || (typeof workspace.clientList === "function" ? workspace.clientList() : []);
function idOf(c) { return String(c.internalId || c.windowId || ""); }
// Never act on the Dock, panels or the desktop.
function usable(c) {
    return !!c
        && (c.normalWindow === undefined || c.normalWindow)
        && !c.specialWindow && !c.dock && !c.desktopWindow
        && String(c.caption || "") !== "Sonoma Dock";
}
let target = wantedId ? (all.find(function (c) { return idOf(c) === wantedId; }) || null) : null;
if (!usable(target)) target = null;
if (!target) target = usable(workspace.activeWindow) ? workspace.activeWindow : null;

function workArea(w) {
    try {
        return workspace.clientArea(KWin.MaximizeArea, w);
    } catch (e) {
        return workspace.clientArea(KWin.MaximizeArea, w.output, workspace.currentDesktop);
    }
}
function place(w, fx, fy, fw, fh) {
    if (!w || w.fullScreen || w.resizeable === false) return;
    if (typeof w.setMaximize === "function") w.setMaximize(false, false);
    const a = workArea(w);
    const x0 = Math.round(a.x + a.width * fx);
    const x1 = Math.round(a.x + a.width * (fx + fw));
    const y0 = Math.round(a.y + a.height * fy);
    const y1 = Math.round(a.y + a.height * (fy + fh));
    w.frameGeometry = { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}
const layouts = {
    "fill": [0, 0, 1, 1],
    "left": [0, 0, 0.5, 1],
    "right": [0.5, 0, 0.5, 1],
    "top": [0, 0, 1, 0.5],
    "bottom": [0, 0.5, 1, 0.5],
    "top-left": [0, 0, 0.5, 0.5],
    "top-right": [0.5, 0, 0.5, 0.5],
    "bottom-left": [0, 0.5, 0.5, 0.5],
    "bottom-right": [0.5, 0.5, 0.5, 0.5]
};

if (action === "minimise") {
    if (target && target.minimizable !== false) target.minimized = true;
} else if (action === "centre") {
    if (target && !target.fullScreen && target.moveable !== false) {
        const a = workArea(target);
        const g = target.frameGeometry;
        target.frameGeometry = {
            x: Math.round(a.x + (a.width - g.width) / 2),
            y: Math.round(a.y + (a.height - g.height) / 2),
            width: g.width,
            height: g.height
        };
    }
} else if (action === "front") {
    const key = target ? String(target.desktopFileName || target.resourceClass || "").toLowerCase() : "";
    all.filter(function (c) {
        const normal = c.normalWindow === undefined || c.normalWindow;
        const same = !key
            || String(c.desktopFileName || c.resourceClass || "").toLowerCase() === key;
        return normal && !c.specialWindow && !c.minimized && same;
    }).forEach(function (c) {
        if (typeof workspace.raiseWindow === "function") {
            workspace.raiseWindow(c);
        } else {
            c.keepAbove = true;
            c.keepAbove = false;
        }
    });
    if (target && !target.minimized) workspace.activeWindow = target;
} else if (layouts[action]) {
    const l = layouts[action];
    place(target, l[0], l[1], l[2], l[3]);
}
"#;

/// Runs a Window-menu action ("minimise", "fill", "centre", "left", "top-left",
/// "front", ...) on the window with `window_id`, or the active window.
pub(crate) fn run_active_window_action(window_id: Option<String>, action: &'static str) {
    thread::spawn(move || {
        let (Ok(id), Ok(action_json)) = (
            serde_json::to_string(&window_id),
            serde_json::to_string(action),
        ) else {
            return;
        };
        let script = ACTIVE_WINDOW_SCRIPT
            .replace("__ID__", &id)
            .replace("__ACTION__", &action_json);
        if let Err(error) = run_kwin_script(&script) {
            eprintln!("sonoma-desktop: KWin could not {action} the window: {error}");
        }
    });
}

pub(crate) fn run_window_action(program: String, action: &'static str) {
    if program == kwin_windows::SETTINGS_IDENTITY {
        settings_window_action(action);
        return;
    }
    if is_wayland_session() {
        thread::spawn(move || {
            if let Err(error) = kwin_window_action(&program, action) {
                eprintln!("sonoma-desktop: KWin could not {action} {program}: {error}");
            }
        });
        return;
    }

    let windows = x11_app_windows(&program);
    let Some(window) = windows.first() else {
        eprintln!("sonoma-desktop: no X11 window found for {program}");
        return;
    };
    let targets: Vec<_> = if action == "activate" {
        vec![window]
    } else {
        windows.iter().collect()
    };
    for window in targets {
        let result = match action {
            "activate" => Command::new("wmctrl").args(["-ia", window]).status(),
            "hide" => Command::new("wmctrl")
                .args(["-ir", window, "-b", "add,hidden"])
                .status(),
            "quit" => Command::new("wmctrl").args(["-ic", window]).status(),
            _ => return,
        };
        if result.is_err() || result.is_ok_and(|status| !status.success()) {
            eprintln!("sonoma-desktop: could not {action} window for {program}");
        }
    }
}

/// The app-name bubble shown above a Dock icon on hover, with an arrow pointing
/// at the icon. There is a single bubble per Dock that *moves* from icon to icon,
/// so sliding the pointer along the Dock never hides and re-shows (and so never
/// fades) a popup.
#[derive(Clone)]
struct HoverLabel {
    popover: gtk::Popover,
    label: gtk::Label,
    parent: gtk::Widget,
    /// Bumped on every show/hide request so a delayed hide can tell it is stale.
    generation: Rc<std::cell::Cell<u64>>,
    current: Rc<RefCell<Option<glib::WeakRef<gtk::Button>>>>,
}

impl HoverLabel {
    fn new(parent: &impl IsA<gtk::Widget>) -> Self {
        let label = gtk::Label::new(None);
        label.add_css_class("sonoma-system-menu");
        label.add_css_class("dock-label");

        let popover = gtk::Popover::new();
        popover.set_position(gtk::PositionType::Top);
        popover.set_has_arrow(true);
        // Sit a little higher above the icon.
        popover.set_offset(0, -6);
        popover.set_autohide(false);
        popover.set_can_target(false);
        popover.set_focusable(false);
        popover.add_css_class("sonoma-menu-popover");
        popover.add_css_class("dock-label-popover");
        popover.set_child(Some(&label));
        popover.set_parent(parent);
        crate::context_menu::attach_blur_region(&popover, label.upcast_ref::<gtk::Widget>(), 8);

        Self {
            popover,
            label,
            parent: parent.as_ref().clone().upcast(),
            generation: Rc::new(std::cell::Cell::new(0)),
            current: Rc::new(RefCell::new(None)),
        }
    }

    fn bump(&self) -> u64 {
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        next
    }

    fn show_for(&self, tile: &gtk::Button, name: &str) {
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

    fn hide_now(&self) {
        self.bump();
        *self.current.borrow_mut() = None;
        self.popover.popdown();
    }

    /// Hides after a moment unless the pointer lands on another icon first.
    fn hide_soon(&self) {
        let token = self.bump();
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(60), move || {
            if this.generation.get() == token {
                this.hide_now();
            }
        });
    }

    fn attach(&self, tile: &gtk::Button, name: &str) {
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
struct PinState {
    pins: Rc<RefCell<HashSet<String>>>,
    /// Pinned apps that are not in dock.toml; stored beside the pins.
    extras: Rc<RefCell<Vec<Entry>>>,
    /// Commands that come from dock.toml.
    configured: Rc<HashSet<String>>,
    /// Set when a pin changed so the refresh loop re-evaluates the tiles.
    dirty: Rc<std::cell::Cell<bool>>,
}

impl PinState {
    fn is_pinned(&self, command: &str) -> bool {
        self.pins.borrow().contains(command)
    }

    fn toggle(&self, entry: &Entry) {
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
            eprintln!("sonoma-desktop: could not save the Dock pins: {error}");
        }
        self.dirty.set(true);
    }
}

/// Replaces the text of a menu button created by `ContextMenu::append_action`.
fn set_item_label(button: &gtk::Button, text: &str) {
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
fn context_menu(
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

struct DockTile {
    button: gtk::Button,
    revealer: gtk::Revealer,
    icon: Option<RoundedIcon>,
    indicator: gtk::Widget,
    program: Option<String>,
    app_aliases: Vec<String>,
    command: String,
    always_open: bool,
    dynamic_identity: Option<String>,
    entry: Entry,
    /// Starts the launch bounce on this tile's icon.
    launch_bounce: Rc<dyn Fn()>,
    /// Like `launch_bounce`, but keeps hopping for at most the given time
    /// (`Duration::ZERO` = a single hop).
    bounce_for: Rc<dyn Fn(Duration)>,
}

struct DockIcon {
    picture: gtk::Picture,
}

fn app_aliases(program: Option<&str>) -> Vec<String> {
    let Some(program) = program else {
        return Vec::new();
    };
    kwin_windows::aliases_for_program(program)
}

fn should_activate_existing(
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

/// Picks the tile's picture: the configured PNG, else the installed
/// application's own icon, else `None` (the caller shows a neutral placeholder).
fn icon_for(entry: &Entry, display: &gdk::Display) -> Option<DockIcon> {
    if let Some(path) = &entry.icon {
        match gdk::Texture::from_filename(path) {
            Ok(texture) => {
                return Some(DockIcon {
                    picture: picture_for(&texture),
                });
            }
            Err(e) => eprintln!(
                "sonoma-desktop: icon {} unavailable for \"{}\" ({e}); using fallback",
                path.display(),
                entry.name
            ),
        }
    }

    let program = program_name(&entry.command)?;
    let theme = gtk::IconTheme::for_display(display);
    if entry.command == kwin_windows::SETTINGS_IDENTITY {
        let name = ["preferences-system", "systemsettings", "org.kde.systemsettings"]
            .into_iter()
            .find(|name| theme.has_icon(name))?;
        let paintable = theme.lookup_icon(
            name,
            &[],
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        return Some(DockIcon {
            picture: picture_for(&paintable),
        });
    }
    if let Some(icon) = installed_app_for(&program).and_then(|app| app.icon()) {
        let paintable = theme.lookup_by_gicon(
            &icon,
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        return Some(DockIcon {
            picture: picture_for(&paintable),
        });
    }
    theme.has_icon(&program).then(|| {
        let paintable = theme.lookup_icon(
            &program,
            &[],
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        DockIcon {
            picture: picture_for(&paintable),
        }
    })
}

fn picture_for(paintable: &impl IsA<gdk::Paintable>) -> gtk::Picture {
    let picture = gtk::Picture::for_paintable(paintable);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_size_request(36, 36);
    picture
}

fn create_tile(
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
                run_window_action(program.clone(), "activate");
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

/// Slides a hidden tile into the Dock and plays the bounce straight after the
/// expansion finishes. `animate_layout` must keep the Dock window (and its
/// blur) following the growing content for the given number of milliseconds.
/// `bounce` is how long the icon may keep hopping (`Duration::ZERO` = one hop).
fn expand_tile(tile: &DockTile, animate_layout: &dyn Fn(u64), bounce: Option<Duration>) {
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

fn create_trash_item() -> (gtk::Button, RoundedIcon) {
    let button = gtk::Button::builder()
        .css_classes(["dock-item", "dock-icon"])
        .focusable(false)
        .build();
    let icon_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/dock-icons/trash.png");
    let picture = match gdk::Texture::from_filename(&icon_path) {
        Ok(texture) => picture_for(&texture),
        Err(error) => {
            eprintln!("sonoma-desktop: supplied Trash icon failed to load: {error}");
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

fn transient_running_entries(
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

fn sync_transient_tiles(
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

/// Updates the panel position once using its newly measured content size.
fn position_sync(
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

fn measured_window_size(window: &gtk::Window, monitor: &gdk::Monitor) -> (i32, i32) {
    let geometry = monitor.geometry();
    let (minimum_width, natural_width, _, _) = window.measure(gtk::Orientation::Horizontal, -1);
    let width = natural_width
        .max(minimum_width)
        .max(1)
        .min(geometry.width());
    let (minimum_height, natural_height, _, _) = window.measure(gtk::Orientation::Vertical, width);
    (width, natural_height.max(minimum_height).max(1))
}

fn resize_window_to_content(window: &gtk::Window, monitor: &gdk::Monitor) {
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
        let displayed_dock_size = Rc::clone(&displayed_dock_size);
        move || {
            let size = tile_size(&monitor, displayed_dock_size.get());
            for tile in size_tiles.borrow().iter() {
                tile.button.set_size_request(size, size);
                if let Some(icon) = &tile.icon {
                    icon.set_icon_size(size - 4);
                }
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
        let snapshot = if windows_changed || pins_changed {
            refresh_tracker.as_ref().map(KWinWindows::snapshot)
        } else {
            None
        };
        let mut layout_changed = false;
        let current_preferences = crate::preferences::current();
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
