//! Desktop environment detection and running-application tracking.

use std::collections::HashSet;

/// Returns the executable names that currently have a Dock app open. Actual
/// window discovery goes through the KWin tracker; this is the degraded
/// fallback for when that bridge could not start.
pub(crate) fn running_apps() -> HashSet<String> {
    running_process_names()
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
