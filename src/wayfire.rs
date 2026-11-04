//! Window tracking and control for the Wayfire compositor, over its IPC socket.
//!
//! Wayfire only offers this when its `ipc`, `ipc-rules` and `wm-actions`
//! plugins are loaded (see `SETUP_HINT`). Messages are JSON documents, each
//! preceded by its length as a 4-byte little-endian integer, sent over the Unix
//! socket named by `$WAYFIRE_SOCKET`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::{json, Value};

use crate::kwin_windows::{self, AppWindow};

pub const SETUP_HINT: &str = "add ipc, ipc-rules and wm-actions to the plugins list in the \
[core] section of wayfire.ini (ipc first), then restart Wayfire";

fn socket_path() -> Option<std::path::PathBuf> {
    std::env::var_os("WAYFIRE_SOCKET").map(Into::into)
}

/// True when this session is running under Wayfire.
pub fn is_session() -> bool {
    socket_path().is_some()
}

/// Sends one request and returns its reply. A fresh connection per call keeps
/// this safe to use from any thread.
fn call(method: &str, data: Value) -> Result<Value, String> {
    let path = socket_path().ok_or("WAYFIRE_SOCKET is not set")?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|error| format!("cannot connect to {}: {error}", path.display()))?;
    let timeout = Some(Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);

    let body = serde_json::to_vec(&json!({ "method": method, "data": data }))
        .map_err(|error| error.to_string())?;
    let length = u32::try_from(body.len()).map_err(|_| "request too large".to_string())?;
    stream
        .write_all(&length.to_le_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|error| format!("{method}: write failed: {error}"))?;

    loop {
        let mut header = [0_u8; 4];
        stream
            .read_exact(&mut header)
            .map_err(|error| format!("{method}: read failed: {error}"))?;
        let length = u32::from_le_bytes(header) as usize;
        if length > 16 * 1024 * 1024 {
            return Err(format!("{method}: reply is unreasonably large"));
        }
        let mut payload = vec![0_u8; length];
        stream
            .read_exact(&mut payload)
            .map_err(|error| format!("{method}: read failed: {error}"))?;
        let value: Value =
            serde_json::from_slice(&payload).map_err(|error| format!("{method}: {error}"))?;
        // Events are pushed on the same socket; only a reply is wanted here.
        if value.get("event").is_some() {
            continue;
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(if error.contains("No such method") {
                format!("{method} is unavailable: {SETUP_HINT}")
            } else {
                format!("{method}: {error}")
            });
        }
        return Ok(value);
    }
}

fn parse_window(view: &Value, focused_id: Option<i64>) -> Option<AppWindow> {
    let id = view.get("id")?.as_i64()?;
    let mapped = view.get("mapped").and_then(Value::as_bool).unwrap_or(true);
    let role = view
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("toplevel")
        .to_ascii_lowercase()
        .replace('_', "-");
    let pid = view.get("pid").and_then(Value::as_i64).unwrap_or(-1);
    // Panels, backgrounds, menus and tooltips are not application windows.
    if !mapped || role != "toplevel" || pid == -1 {
        return None;
    }
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| view.get(*key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string()
    };
    let flag = |key: &str| view.get(key).and_then(Value::as_bool).unwrap_or(false);
    Some(AppWindow {
        window_id: id.to_string(),
        title: text(&["title"]),
        desktop_file: text(&["app-id", "app_id"]),
        resource_class: String::new(),
        resource_name: String::new(),
        pid,
        minimized: flag("minimized"),
        focused: flag("activated") || focused_id == Some(id),
        special: false,
    })
}

fn focused_view_id() -> Option<i64> {
    call("window-rules/get-focused-view", json!({}))
        .ok()?
        .get("info")?
        .get("id")?
        .as_i64()
}

/// Every mapped application window.
pub fn list_windows() -> Result<Vec<AppWindow>, String> {
    let views = call("window-rules/list-views", json!({}))?;
    let views = views
        .as_array()
        .ok_or("window-rules/list-views returned something unexpected")?;
    let focused = focused_view_id();
    Ok(views
        .iter()
        .filter_map(|view| parse_window(view, focused))
        .collect())
}

fn view_number(id: &str) -> Result<i64, String> {
    id.parse::<i64>().map_err(|_| format!("bad window id {id:?}"))
}

fn set_minimized(id: &str, state: bool) -> Result<(), String> {
    call(
        "wm-actions/set-minimized",
        json!({ "view_id": view_number(id)?, "state": state }),
    )
    .map(drop)
}

fn focus(id: &str) -> Result<(), String> {
    call("window-rules/focus-view", json!({ "id": view_number(id)? })).map(drop)
}

fn close(id: &str) -> Result<(), String> {
    call("window-rules/close-view", json!({ "id": view_number(id)? })).map(drop)
}

/// Dock actions on every window of `program`: "activate", "activate-visible",
/// "hide" (minimise) and "quit" (close).
pub fn window_action(program: &str, action: &str) -> Result<(), String> {
    let aliases = kwin_windows::aliases_for_program(program);
    let windows: Vec<AppWindow> = list_windows()?
        .into_iter()
        .filter(|window| window.matches(&aliases))
        .collect();
    let preferred = || {
        windows
            .iter()
            .find(|window| window.focused)
            .or_else(|| windows.iter().find(|window| !window.minimized))
    };
    match action {
        "activate" => {
            if let Some(window) = preferred().or_else(|| windows.first()) {
                if window.minimized {
                    set_minimized(&window.window_id, false)?;
                }
                focus(&window.window_id)?;
            }
        }
        // Minimised windows have their own Dock tiles: leave them minimised.
        "activate-visible" => {
            if let Some(window) = preferred() {
                focus(&window.window_id)?;
            }
        }
        "hide" => {
            for window in &windows {
                set_minimized(&window.window_id, true)?;
            }
        }
        "quit" => {
            for window in &windows {
                close(&window.window_id)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Window-menu actions on the window `window_id`, or the focused window.
pub fn active_window_action(window_id: Option<String>, action: &str) -> Result<(), String> {
    let focused = || focused_view_id().map(|id| id.to_string());
    match action {
        "minimise" => {
            if let Some(id) = window_id.or_else(focused) {
                set_minimized(&id, true)?;
            }
        }
        // Only ever the window that was asked for.
        "restore" => {
            if let Some(id) = window_id {
                set_minimized(&id, false)?;
                focus(&id)?;
            }
        }
        "front" => {
            if let Some(id) = window_id.or_else(focused) {
                focus(&id)?;
            }
        }
        other => {
            return Err(format!("the \"{other}\" window action is not supported on Wayfire yet"));
        }
    }
    Ok(())
}

/// Wayfire's blur plugin blurs whole windows chosen by this rule: normal
/// windows, plus the shell's menu bar and dock. Wayfire reports a layer-shell
/// surface's namespace as its app-id (the dock is classed as a "panel", the menu
/// bar as an "overlay"), so they are matched by name. The wallpaper is left out.
const BLUR_RULE: &str =
    r#"type is "toplevel" | app_id is "lunar-menubar" | app_id is "lunar-dock""#;

/// Turns on backdrop blur for the shell, in the running compositor only (the
/// change is not written to wayfire.ini). Set `LUNAR_NO_WAYFIRE_BLUR` to skip it.
///
/// Wayfire applies the rule when a window first appears, so this runs before
/// the shell creates its surfaces. For a setting that survives restarts, put the
/// same rule under `[blur]` in wayfire.ini (see the README).
pub fn setup_blur() {
    if std::env::var_os("LUNAR_NO_WAYFIRE_BLUR").is_some() {
        return;
    }
    match call(
        "wayfire/set-config-options",
        json!({ "blur/blur_by_default": BLUR_RULE }),
    ) {
        Ok(reply) => eprintln!("[wayfire-blur] set blur_by_default = {BLUR_RULE} (reply: {reply})"),
        Err(error) => eprintln!(
            "[wayfire-blur] could not set the blur rule: {error} \
             (is `blur` in the plugins list of wayfire.ini?)"
        ),
    }
    // Describe what Wayfire sees, so the rule can be corrected from the log.
    std::thread::spawn(|| {
        let plugins = call("wayfire/get-config-option", json!({ "option": "core/plugins" }));
        eprintln!("[wayfire-blur] core/plugins: {plugins:?}");
        std::thread::sleep(Duration::from_secs(3));
        let Ok(views) = call("window-rules/list-views", json!({})) else {
            return;
        };
        for view in views.as_array().into_iter().flatten() {
            let app_id = view.get("app-id").and_then(Value::as_str).unwrap_or_default();
            if app_id.starts_with("lunar-") {
                eprintln!(
                    "[wayfire-blur] {app_id}: type={} layer={}",
                    view.get("type").and_then(Value::as_str).unwrap_or("?"),
                    view.get("layer").and_then(Value::as_str).unwrap_or("?"),
                );
            }
        }
    });
}
