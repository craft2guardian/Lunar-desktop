//! Desktop environment detection and running-application tracking.

use std::collections::HashSet;
use std::process::Command;

#[derive(Clone, Copy)]
pub(crate) enum DesktopKind {
    Hyprland,
    Kde,
    Other,
}

pub(crate) fn detect_desktop() -> DesktopKind {
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
pub(crate) fn running_apps(desktop: DesktopKind) -> HashSet<String> {
    match desktop {
        DesktopKind::Hyprland => hyprland_window_classes().unwrap_or_else(running_process_names),
        // The persistent KWin tracker is the normal KDE source. Retain this
        // as a degraded fallback only when that bridge could not start.
        DesktopKind::Kde => running_process_names(),
        DesktopKind::Other => HashSet::new(),
    }
}

/// Read Hyprland's current client list so indicators represent actual windows.
pub(crate) fn hyprland_window_classes() -> Option<HashSet<String>> {
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
pub(crate) fn running_process_names() -> HashSet<String> {
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

pub(crate) fn app_is_running(program: &str, running: &HashSet<String>) -> bool {
    let program = program.to_ascii_lowercase();
    running.iter().any(|name| {
        name == &program
            || name.strip_suffix(".desktop") == Some(program.as_str())
            || name.ends_with(&format!(".{program}"))
    })
}
