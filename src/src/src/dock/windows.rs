//! KWin / X11 window control scripts and helpers.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use crate::kwin_windows;
use super::launcher::settings_window_action;

pub(crate) fn x11_app_windows(program: &str) -> Vec<String> {
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

pub(crate) fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub(crate) static KWIN_ACTION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn kwin_window_action(program: &str, action: &str) -> Result<(), String> {
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
}} else if (action === "activate-visible") {{
    // Minimised windows have their own Dock tiles: leave them minimised.
    const window = matches.find(function (candidate) {{ return candidate.active; }})
        || matches.find(function (candidate) {{ return !candidate.minimized; }});
    if (window && !window.active) {{
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
} else if (action === "restore") {
    // Only ever the window that was asked for, never the active-window fallback.
    const wanted = wantedId ? all.find(function (c) { return idOf(c) === wantedId; }) : null;
    if (usable(wanted)) {
        wanted.minimized = false;
        workspace.activeWindow = wanted;
    }
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
pub fn run_active_window_action(window_id: Option<String>, action: &'static str) {
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

pub fn run_window_action(program: String, action: &'static str) {
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
    let targets: Vec<_> = if matches!(action, "activate" | "activate-visible") {
        vec![window]
    } else {
        windows.iter().collect()
    };
    for window in targets {
        let result = match action {
            "activate" | "activate-visible" => {
                Command::new("wmctrl").args(["-ia", window]).status()
            }
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
