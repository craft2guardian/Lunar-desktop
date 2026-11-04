//! Dock configuration: reads `config/dock.toml` (see that file for the format).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Used when no config file is found or the found one is unusable.
const DEFAULT_CONFIG: &str = include_str!("../config/dock.toml");

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub icon: Option<PathBuf>,
    pub command: String,
}

/// The bundled/configured entries start pinned. Once the user changes a pin,
/// the resulting list is stored separately from the editable dock.toml.
pub fn load_pins(entries: &[Entry]) -> HashSet<String> {
    let Some(path) = pins_path() else {
        return entries.iter().map(|entry| entry.command.clone()).collect();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return entries.iter().map(|entry| entry.command.clone()).collect();
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        eprintln!("sonoma-desktop: invalid dock pin configuration; using configured entries");
        return entries.iter().map(|entry| entry.command.clone()).collect();
    };
    table
        .get("pins")
        .and_then(toml::Value::as_array)
        .map(|pins| {
            pins.iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_else(|| entries.iter().map(|entry| entry.command.clone()).collect())
}

/// Extra Dock entries the user added through the Dock menu ("Add to Dock") for
/// applications that are not listed in dock.toml. They live beside the pins.
pub fn load_pinned_extras() -> Vec<Entry> {
    let Some(path) = pins_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse(&text, None).unwrap_or_default()
}

/// Persists the pin list plus the extra entries added from the Dock menu.
pub fn save_pins(pins: &HashSet<String>, extras: &[Entry]) -> Result<(), String> {
    let path = pins_path().ok_or("could not determine the configuration directory")?;
    let parent = path.parent().ok_or("invalid dock pin configuration path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut sorted: Vec<&String> = pins.iter().collect();
    sorted.sort();
    let values = sorted
        .iter()
        .map(|pin| format!("  {}", toml::Value::String((*pin).clone())))
        .collect::<Vec<_>>()
        .join(",\n");
    let mut contents = format!("pins = [\n{values}\n]\n");
    for entry in extras {
        contents.push_str("\n[[app]]\n");
        contents.push_str(&format!(
            "name = {}\n",
            toml::Value::String(entry.name.clone())
        ));
        contents.push_str(&format!(
            "command = {}\n",
            toml::Value::String(entry.command.clone())
        ));
        if let Some(icon) = entry.icon.as_ref().and_then(|icon| icon.to_str()) {
            contents.push_str(&format!("icon = {}\n", toml::Value::String(icon.to_string())));
        }
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, contents).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn pins_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("sonoma-desktop/dock-pins.toml"))
}

/// Loads the Dock entries. Never fails: problems are reported on stderr and the
/// bundled default configuration is used instead.
pub fn load_dock() -> Vec<Entry> {
    let (text, base) = match find_file() {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => (text, path.parent().map(Path::to_path_buf)),
            Err(e) => {
                eprintln!("sonoma-desktop: cannot read {}: {e}", path.display());
                (DEFAULT_CONFIG.to_string(), None)
            }
        },
        None => (DEFAULT_CONFIG.to_string(), None),
    };

    parse(&text, base.as_deref()).unwrap_or_else(|e| {
        eprintln!("sonoma-desktop: invalid dock config ({e}); using the built-in default");
        parse(DEFAULT_CONFIG, None).unwrap_or_default()
    })
}

/// Search order: $SONOMA_DOCK_CONFIG, ./config/dock.toml, next to the built
/// binary's project folder, ~/.config/sonoma-desktop/dock.toml, and finally the
/// folder the project was built from.
fn find_file() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("SONOMA_DOCK_CONFIG") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("config/dock.toml"));
    if let Ok(exe) = std::env::current_exe() {
        // <project>/target/<profile>/sonoma-desktop
        if let Some(project) = exe.ancestors().nth(3) {
            candidates.push(project.join("config/dock.toml"));
        }
    }
    if let Some(dir) = config_home() {
        candidates.push(dir.join("sonoma-desktop/dock.toml"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("config/dock.toml"));
    candidates.into_iter().find(|p| p.is_file())
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn parse(text: &str, base: Option<&Path>) -> Result<Vec<Entry>, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let apps = table
        .get("app")
        .and_then(|v| v.as_array())
        .ok_or("no [[app]] entries")?;

    let mut entries = Vec::new();
    for (i, app) in apps.iter().enumerate() {
        let get = |key: &str| {
            app.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let Some(command) = get("command") else {
            eprintln!(
                "sonoma-desktop: dock entry {} has no command; skipped",
                i + 1
            );
            continue;
        };
        entries.push(Entry {
            name: get("name")
                .or_else(|| command.split_whitespace().next())
                .unwrap_or("App")
                .to_string(),
            icon: get("icon").map(|p| resolve_icon_path(p, base)),
            command: command.to_string(),
        });
    }
    Ok(entries)
}

/// Absolute and `~/` paths are used as given. Relative ones are looked up next
/// to the config file, then in the project folder above it.
fn resolve_icon_path(path: &str, base: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home() {
            return home.join(rest);
        }
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let Some(base) = base else {
        return path.to_path_buf();
    };
    let candidates = [Some(base), base.parent()];
    let mut candidates = candidates.into_iter().flatten().map(|dir| dir.join(path));
    let first = candidates.next().unwrap_or_else(|| path.to_path_buf());
    std::iter::once(first.clone())
        .chain(candidates)
        .find(|p| p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_in_order_and_skips_commandless() {
        let text = r#"
            [[app]]
            name = "A"
            command = "foo --bar"
            [[app]]
            name = "NoCommand"
            [[app]]
            command = "baz"
            icon = "/abs/icon.png"
        "#;
        let e = parse(text, None).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(
            (e[0].name.as_str(), e[0].command.as_str()),
            ("A", "foo --bar")
        );
        assert_eq!(e[1].name, "baz"); // name defaults to the program
        assert_eq!(e[1].icon.as_deref(), Some(Path::new("/abs/icon.png")));
    }

    #[test]
    fn bad_toml_is_an_error_and_bundled_default_parses() {
        assert!(parse("[[app", None).is_err());
        assert!(!parse(DEFAULT_CONFIG, None).unwrap().is_empty());
    }
}
