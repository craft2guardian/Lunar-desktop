//! Live application-window state from KWin's scripting API.
//!
//! KWin owns the authoritative window list on both Wayland and X11. This
//! module loads one small persistent script and receives its snapshots over
//! the session D-Bus connection, so Dock state follows real windows rather
//! than process names.

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Output};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::gio;

const OBJECT_PATH: &str = "/dev/lunar/DockWindowTracker";
const INTERFACE: &str = "dev.lunar.DockWindowTracker";

#[derive(Clone, Debug, Default)]
pub struct WindowSnapshot {
    pub windows: Vec<AppWindow>,
    pub revision: u64,
    pub event: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub window_id: String,
    pub title: String,
    pub desktop_file: String,
    pub resource_class: String,
    pub resource_name: String,
    pub pid: i64,
    pub minimized: bool,
    pub focused: bool,
    pub special: bool,
}

/// Dock identity (and Dock command) of this program's own System Settings
/// window. It lives in the shell's process, so it is matched by title.
pub const SETTINGS_IDENTITY: &str = "dev.lunar.settings";

impl AppWindow {
    /// True for the System Settings window opened by this very program.
    pub fn is_lunar_settings(&self) -> bool {
        self.pid > 0
            && self.pid == i64::from(std::process::id())
            && self.title.trim() == crate::settings::WINDOW_TITLE
    }

    /// Stable KWin application identity. App IDs are authoritative; class and
    /// resource name are only used for windows whose app ID is empty.
    pub fn resolved_identity(&self) -> String {
        if self.is_lunar_settings() {
            return SETTINGS_IDENTITY.to_string();
        }
        let app_id = normalize(&self.desktop_file);
        if !app_id.is_empty() {
            return app_id;
        }
        let class = normalize(&self.resource_class);
        if !class.is_empty() {
            return class;
        }
        let name = normalize(&self.resource_name);
        if !name.is_empty() {
            return name;
        }
        format!("pid:{}", self.pid)
    }

    /// KWin also reports desktop and shell surfaces as windows. They must not
    /// create app indicators, but minimized application windows remain valid.
    pub fn is_trackable_app(&self) -> bool {
        if self.special {
            return false;
        }
        // System Settings is a real app with its own Dock tile.
        if self.is_lunar_settings() {
            return true;
        }
        // Windows of this program (Settings, About, the Dock itself, ...)
        // never get a Dock tile.
        if self.pid > 0 && self.pid == i64::from(std::process::id()) {
            return false;
        }
        let identifiers = [
            normalize(&self.desktop_file),
            normalize(&self.resource_class),
            normalize(&self.resource_name),
        ];
        !identifiers.iter().any(|id| {
            id == "dev.lunar"
                || id.starts_with("dev.lunar.")
                || id == "lunar-desktop"
                || id == "lunar_desktop"
                || id == "org.kde.plasmashell"
                || id == "plasmashell"
                || id.starts_with("org.kde.plasma.")
        })
    }

    /// Stable KWin application identity for grouping windows.
    pub fn group_key(&self) -> String {
        self.resolved_identity()
    }

    pub fn matches(&self, aliases: &[String]) -> bool {
        if !self.is_trackable_app() {
            return false;
        }
        let app_id = normalize(&self.desktop_file);
        let values = if self.is_lunar_settings() {
            vec![SETTINGS_IDENTITY.to_string()]
        } else if !app_id.is_empty() {
            // Never let a conflicting WM_CLASS override a populated app ID.
            vec![app_id]
        } else {
            [
                normalize(&self.resource_class),
                normalize(&self.resource_name),
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect()
        };
        aliases.iter().any(|alias| {
            let alias = normalize(alias);
            !alias.is_empty()
                && values.iter().any(|value| {
                    value == &alias || value.strip_suffix(&format!(".{alias}")).is_some()
                })
        })
    }
}

#[derive(Clone)]
pub struct KWinWindows {
    inner: Rc<TrackerInner>,
}

struct TrackerInner {
    snapshot: Rc<RefCell<WindowSnapshot>>,
    /// Only present when tracking through KWin.
    kde: Option<KdeParts>,
}

struct KdeParts {
    // Keep the registered D-Bus method alive for the lifetime of the tracker.
    _connection: gio::DBusConnection,
    _node: gio::DBusNodeInfo,
    _registration: gio::RegistrationId,
    _script_id: i32,
    plugin: String,
}

impl KWinWindows {
    /// Starts a persistent KWin script and returns the receiving snapshot.
    /// Failure is reported so the caller can retain a safe, limited fallback.
    pub fn start() -> Result<Self, String> {
        eprintln!("[dock-track-debug][tracker-starting] connecting to session D-Bus and registering the window snapshot receiver");
        let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
            .map_err(|error| format!("session D-Bus is unavailable: {error}"))?;
        let service = connection
            .unique_name()
            .ok_or_else(|| "session D-Bus did not assign a unique name".to_string())?
            .to_string();
        let node = gio::DBusNodeInfo::for_xml(&format!(
            "<node><interface name='{INTERFACE}'><method name='UpdateWindows'><arg type='s' direction='in'/><arg type='s' direction='in'/></method></interface></node>"
        ))
        .map_err(|error| error.to_string())?;
        let interface = node
            .lookup_interface(INTERFACE)
            .ok_or_else(|| "could not define KWin window tracker D-Bus interface".to_string())?;
        let snapshot = Rc::new(RefCell::new(WindowSnapshot::default()));
        let receiving_snapshot = Rc::clone(&snapshot);
        let _registration = connection
            .register_object(OBJECT_PATH, &interface)
            .method_call(move |_, _, _, _, method, parameters, invocation| {
                if method == "UpdateWindows" {
                    if let Some((json, reason)) = parameters.get::<(String, String)>() {
                        match parse_snapshot(&json) {
                            Ok(windows) => {
                                let details: Vec<_> = windows
                                    .iter()
                                    .map(|window| {
                                        format!(
                                            "id={:?} title={:?} app_id={:?} resource_class={:?} resource_name={:?} pid={} minimized={} focused={} special={}",
                                            window.window_id,
                                            window.title,
                                            window.desktop_file,
                                            window.resource_class,
                                            window.resource_name,
                                            window.pid,
                                            window.minimized,
                                            window.focused,
                                            window.special
                                        )
                                    })
                                    .collect();
                                eprintln!(
                                    "[dock-track-debug][dbus-receive] event={reason} count={} windows={details:?}",
                                    windows.len()
                                );
                                let mut groups = std::collections::BTreeMap::<String, usize>::new();
                                for window in &windows {
                                    let identity = window.resolved_identity();
                                    let trackable = window.is_trackable_app();
                                    eprintln!("[dock-track-debug][window-identity] event={reason} id={:?} identity={identity:?} app_id={:?} resource_class={:?} resource_name={:?} minimized={} trackable={trackable}", window.window_id, window.desktop_file, window.resource_class, window.resource_name, window.minimized);
                                    if trackable {
                                        *groups.entry(identity).or_default() += 1;
                                    }
                                }
                                eprintln!("[dock-track-debug][grouped-running-apps] event={reason} groups={groups:?}");
                                let mut snapshot = receiving_snapshot.borrow_mut();
                                snapshot.windows = windows;
                                snapshot.revision = snapshot.revision.wrapping_add(1);
                                snapshot.event = reason;
                            }
                            Err(error) => {
                                eprintln!(
                                    "[dock-track-debug][dbus-parse-error] event={reason} error={error} raw={json}"
                                )
                            }
                        }
                    } else {
                        eprintln!("[dock-track-debug][dbus-signature-error] unexpected parameters={parameters:?}");
                    }
                }
                invocation.return_value(None);
            })
            .build()
            .map_err(|error| error.to_string())?;
        eprintln!(
            "[dock-track-debug][dbus-receiver-ready] unique_name={service} object={OBJECT_PATH} interface={INTERFACE}"
        );

        let (script_id, plugin) = load_tracker_script(&service).map_err(|error| {
            eprintln!("[dock-track-debug][script-start-failed] {error}");
            error
        })?;
        eprintln!("[dock-track-debug][tracker-ready] plugin={plugin} script_id={script_id} dbus_service={service} object={OBJECT_PATH}");
        Ok(Self {
            inner: Rc::new(TrackerInner {
                snapshot,
                kde: Some(KdeParts {
                    _connection: connection,
                    _node: node,
                    _registration,
                    _script_id: script_id,
                    plugin,
                }),
            }),
        })
    }

    pub fn snapshot(&self) -> WindowSnapshot {
        self.inner.snapshot.borrow().clone()
    }

    pub fn revision(&self) -> u64 {
        self.inner.snapshot.borrow().revision
    }
}

impl Drop for TrackerInner {
    fn drop(&mut self) {
        let Some(kde) = &self.kde else {
            return;
        };
        let Some(qdbus) = ["qdbus6", "qdbus"]
            .into_iter()
            .find(|name| Command::new(name).arg("--help").output().is_ok())
        else {
            return;
        };
        let _ = Command::new(qdbus)
            .args([
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting.unloadScript",
            ])
            .arg(&kde.plugin)
            .output();
    }
}

pub fn normalize_identifier(value: &str) -> String {
    let value = value.trim();
    let value = value.rsplit('/').next().unwrap_or(value);
    let value = value.to_ascii_lowercase();
    value.strip_suffix(".desktop").unwrap_or(&value).to_string()
}

/// The same app identity aliases used by Dock tiles, derived from the launch
/// program and its installed desktop entry when one is available.
pub fn aliases_for_program(program: &str) -> Vec<String> {
    let mut aliases = vec![normalize_identifier(program)];
    if let Some(id) = installed_app_for_program(program).and_then(|app| app.id()) {
        let id = normalize_identifier(&id);
        if !aliases.contains(&id) {
            aliases.push(id);
        }
    }
    aliases
}

pub fn installed_app_for_program(program: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::all()
        .into_iter()
        .find(|app| app_executable_matches(app, program))
}

/// Finds an installed desktop entry for a KWin identity. Exact desktop IDs
/// win; a final identifier component handles IDs such as `org.kde.ark` when
/// the window manager reports only the `ark` class.
#[allow(dead_code)]
pub fn installed_app_for_identity(identity: &str) -> Option<gio::AppInfo> {
    let identity = normalize_identifier(identity);
    if identity.is_empty() {
        return None;
    }
    gio::AppInfo::all().into_iter().find(|app| {
        app.id().is_some_and(|id| {
            let id = normalize_identifier(&id);
            id == identity || id.ends_with(&format!(".{identity}"))
        })
    })
}

fn app_executable_matches(app: &gio::AppInfo, program: &str) -> bool {
    unsafe {
        let executable = gio::ffi::g_app_info_get_executable(app.as_ptr());
        if executable.is_null() {
            return false;
        }
        Path::new(std::ffi::OsStr::from_bytes(
            CStr::from_ptr(executable).to_bytes(),
        ))
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == program)
    }
}

/// Aggregate matched windows into one state per application identity.
/// Minimized windows count as running; focus is true if any app window is active.
pub fn matching_window_state(aliases: &[String], snapshot: &WindowSnapshot) -> (bool, bool, bool) {
    let groups = matching_window_groups(aliases, snapshot);
    (
        !groups.is_empty(),
        groups.values().flatten().any(|window| window.focused),
        !groups.is_empty()
            && groups
                .values()
                .all(|windows| windows.iter().all(|window| window.minimized)),
    )
}

/// Match windows to one Dock entry and group multiple windows by resolved
/// application identity. Both the CLI and GTK Dock use this same grouping.
pub fn matching_window_groups<'a>(
    aliases: &[String],
    snapshot: &'a WindowSnapshot,
) -> std::collections::BTreeMap<String, Vec<&'a AppWindow>> {
    let mut groups = std::collections::BTreeMap::<String, Vec<&AppWindow>>::new();
    for window in snapshot
        .windows
        .iter()
        .filter(|window| window.matches(aliases))
    {
        groups.entry(window.group_key()).or_default().push(window);
    }
    groups
}

fn normalize(value: &str) -> String {
    normalize_identifier(value)
}

fn parse_snapshot(json: &str) -> Result<Vec<AppWindow>, String> {
    let values: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(values
        .into_iter()
        .map(|value| AppWindow {
            window_id: string_field(&value, "windowId"),
            title: string_field(&value, "title"),
            desktop_file: string_field(&value, "applicationId"),
            resource_class: string_field(&value, "resourceClass"),
            resource_name: string_field(&value, "resourceName"),
            pid: value
                .get("pid")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or_default(),
            minimized: value
                .get("minimized")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            focused: value
                .get("focused")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            special: value
                .get("special")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
        .collect())
}

fn string_field(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn load_tracker_script(service: &str) -> Result<(i32, String), String> {
    let qdbus = ["qdbus6", "qdbus"]
        .into_iter()
        .find(|name| Command::new(name).arg("--help").output().is_ok())
        .ok_or("qdbus is not installed")?;
    let plugin = format!("lunar_dock_window_tracker_{}", std::process::id());
    let path = std::env::temp_dir().join(format!("{plugin}.js"));
    let service = serde_json::to_string(service).map_err(|error| error.to_string())?;
    let object_path = serde_json::to_string(OBJECT_PATH).map_err(|error| error.to_string())?;
    let interface = serde_json::to_string(INTERFACE).map_err(|error| error.to_string())?;
    let script = format!(
        r#"const targetService = {service};
const targetPath = {object_path};
const targetInterface = {interface};
var watchedWindows = [];
function allWindows() {{
    if (workspace.stackingOrder) return workspace.stackingOrder;
    if (typeof workspace.windowList === "function") return workspace.windowList();
    if (typeof workspace.clientList === "function") return workspace.clientList();
    return [];
}}
function publishWindows(reason) {{
    const windows = allWindows().map(function (window) {{
        return {{
            windowId: String(window.internalId || window.windowId || ""),
            title: String(window.caption || ""),
            applicationId: String(window.desktopFileName || ""),
            resourceClass: String(window.resourceClass || ""),
            resourceName: String(window.resourceName || ""),
            pid: Number(window.pid || 0),
            minimized: Boolean(window.minimized),
            focused: Boolean(window.active),
            special: Boolean(window.specialWindow)
        }};
    }});
    const payload = JSON.stringify(windows);
    print("[dock-track-debug][kwin-event] event=" + reason + " count=" + windows.length + " windows=" + payload);
    callDBus(targetService, targetPath, targetInterface, "UpdateWindows", payload, reason);
}}
function watchWindow(window) {{
    if (watchedWindows.indexOf(window) >= 0) return;
    watchedWindows.push(window);
    window.minimizedChanged.connect(function () {{ publishWindows("minimizedChanged"); }});
    window.desktopFileNameChanged.connect(function () {{ publishWindows("desktopFileNameChanged"); }});
    window.windowClassChanged.connect(function () {{ publishWindows("windowClassChanged"); }});
    window.activeChanged.connect(function () {{ publishWindows("activeChanged"); }});
}}
allWindows().forEach(watchWindow);
workspace.windowAdded.connect(function (window) {{ watchWindow(window); publishWindows("windowAdded"); }});
workspace.windowRemoved.connect(function () {{ publishWindows("windowRemoved"); }});
workspace.windowActivated.connect(function () {{ publishWindows("windowActivated"); }});
workspace.currentDesktopChanged.connect(function () {{ publishWindows("currentDesktopChanged"); }});
print("[dock-track-debug][kwin-script-started] enumerator=workspace.stackingOrder");
publishWindows("initial");
"#
    );
    std::fs::write(&path, script).map_err(|error| error.to_string())?;
    let loaded = match Command::new(qdbus)
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
        return Err(command_failure(&loaded));
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
        return Err("KWin rejected the window tracker script".to_string());
    }
    eprintln!(
        "[dock-track-debug][script-loaded] plugin={plugin} script_id={id} source={}",
        path.display()
    );
    let script_path = format!("/Scripting/Script{id}");
    // KWin loads the JavaScript asynchronously while handling run(), so keep
    // the temporary source available until the D-Bus call has completed.
    let run = Command::new(qdbus)
        .args(["org.kde.KWin", &script_path, "org.kde.kwin.Script.run"])
        .output();
    let _ = std::fs::remove_file(&path);
    if let Ok(output) = &run {
        if output.status.success() {
            eprintln!("[dock-track-debug][script-run-returned] plugin={plugin} script_id={id} success=true");
            return Ok((id, plugin));
        }
    }
    let _ = Command::new(qdbus)
        .args([
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.unloadScript",
        ])
        .arg(&plugin)
        .output();
    let error = match run {
        Ok(output) => command_failure(&output),
        Err(error) => error.to_string(),
    };
    eprintln!("[dock-track-debug][script-run-returned] plugin={plugin} script_id={id} success=false error={error}");
    Err(format!(
        "KWin did not start the window tracker script: {error}"
    ))
}

pub fn command_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("{stderr} ({stdout})"),
        (false, true) => stderr,
        (true, false) => stdout,
        (true, true) => format!("qdbus exited with {}", output.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_windows_and_keeps_focus_and_minimize_state() {
        let windows = parse_snapshot(
            r#"[
                {"windowId":"{a}","title":"konsole","applicationId":"org.kde.konsole","resourceClass":"konsole","resourceName":"konsole","pid":10,"minimized":true,"focused":false,"special":false},
                {"windowId":"{b}","title":"Konsole — shell","applicationId":"org.kde.konsole","resourceClass":"konsole","resourceName":"konsole","pid":11,"minimized":false,"focused":true,"special":false},
                {"windowId":"{c}","title":"Desktop","resourceClass":"plasmashell","special":true}
            ]"#,
        )
        .unwrap();
        assert_eq!(windows.len(), 3);
        assert!(windows[0].minimized);
        assert!(windows[1].focused);
        assert_eq!(windows[1].title, "Konsole — shell");
        assert_eq!(windows[0].group_key(), windows[1].group_key());
        assert!(windows[0].matches(&["konsole".to_string()]));
        assert!(!windows[2].matches(&["konsole".to_string()]));
    }

    #[test]
    fn app_id_has_priority_and_empty_app_id_falls_back_to_window_class() {
        let populated = AppWindow {
            window_id: "1".into(),
            title: "Ark".into(),
            desktop_file: "org.kde.ark.desktop".into(),
            resource_class: "unrelated-class".into(),
            resource_name: "unrelated-name".into(),
            pid: 1,
            minimized: false,
            focused: false,
            special: false,
        };
        assert!(populated.matches(&["ark".into()]));
        assert!(!populated.matches(&["unrelated-class".into()]));
        assert_eq!(populated.resolved_identity(), "org.kde.ark");

        let fallback = AppWindow {
            desktop_file: String::new(),
            resource_class: "Ark".into(),
            ..populated
        };
        assert!(fallback.matches(&["ark".into()]));
        assert_eq!(fallback.resolved_identity(), "ark");
    }

    #[test]
    fn filters_internal_windows_but_keeps_minimized_application_windows() {
        let own = AppWindow {
            desktop_file: "dev.lunar.Desktop".into(),
            ..AppWindow {
                window_id: "own".into(),
                title: "Dock".into(),
                desktop_file: String::new(),
                resource_class: String::new(),
                resource_name: String::new(),
                pid: 2,
                minimized: false,
                focused: false,
                special: false,
            }
        };
        let mut plasma = own.clone();
        plasma.desktop_file.clear();
        plasma.resource_class = "plasmashell".into();
        let mut minimized = own.clone();
        minimized.desktop_file = "org.kde.ark".into();
        minimized.minimized = true;
        assert!(!own.is_trackable_app());
        assert!(!plasma.is_trackable_app());
        assert!(minimized.is_trackable_app());
        assert!(minimized.matches(&["ark".into()]));
    }

    #[test]
    fn multiple_windows_for_one_app_are_grouped_once_and_minimized_still_runs() {
        let windows = parse_snapshot(
            r#"[
                {"windowId":"ark-1","applicationId":"org.kde.ark.desktop","resourceClass":"Ark","minimized":true},
                {"windowId":"ark-2","applicationId":"org.kde.ark","resourceClass":"Ark","focused":true}
            ]"#,
        )
        .unwrap();
        let snapshot = WindowSnapshot {
            windows,
            ..WindowSnapshot::default()
        };
        let groups = matching_window_groups(&["ark".into()], &snapshot);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups["org.kde.ark"].len(), 2);
        assert_eq!(
            matching_window_state(&["ark".into()], &snapshot),
            (true, true, false)
        );
    }
}
