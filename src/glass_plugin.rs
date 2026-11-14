//! Controls the KWin glass plugin (kwin-effects-glass) from Settings.
//!
//! Lunar paints no glass of its own on the Dock; the plugin supplies the blur.
//! One Liquid Glass slider in Settings sets both of the plugin's strengths,
//! "Content blur" and "Docks blur", by writing them into `kwinrc` and asking
//! KWin to reload the effect, which is what the plugin's own settings page does.
//!
//! Group and key names come from the plugin's `src/blur.kcfg`. If a plugin
//! update renames them, change the constants below.

use std::process::{Command, Stdio};

/// `kwinrc` group of the plugin (`<group name=…>` in blur.kcfg).
const GROUP: &str = "Effect-blurplus";
/// The effect's id as KWin knows it (used to reload just this effect); it is
/// what `qdbus-qt6 org.kde.KWin /Effects loadedEffects` lists. It differs from
/// the config group's name above: the group comes from blur.kcfg.
const EFFECT_ID: &str = "glass";
/// The two keys the slider drives: Content blur and Docks blur (both Int).
const KEYS: [&str; 2] = ["BlurStrength", "DockBlurStrength"];
/// Plugin strength at the Light and Strong ends of the slider. The plugin
/// allows more, but the difference stops being noticeable above about 4, so
/// the whole slider covers just the useful part.
const MIN_STRENGTH: i32 = 1;
const MAX_STRENGTH: i32 = 6;

/// Maps the 0–100 slider onto the plugin's strength range.
pub fn strength_for(percent: f64) -> i32 {
    let t = percent.clamp(0.0, 100.0) / 100.0;
    MIN_STRENGTH + (t * f64::from(MAX_STRENGTH - MIN_STRENGTH)).round() as i32
}

/// Writes both strengths and tells KWin to reload. Runs a few short helper
/// processes, so call it off the UI thread or debounced.
pub fn apply(percent: f64) {
    let value = strength_for(percent).to_string();
    for key in KEYS {
        let wrote = ["kwriteconfig6", "kwriteconfig5"].iter().any(|program| {
            Command::new(program)
                .args(["--file", "kwinrc", "--group", GROUP, "--key", key, &value])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        });
        if !wrote {
            eprintln!("lunar-desktop: could not write {GROUP}/{key} to kwinrc (is kwriteconfig6 installed?)");
            return;
        }
    }
    // System Settings reloads one effect with Effects.reconfigureEffect; a
    // plain KWin reconfigure does not re-read an effect's own settings.
    if !dbus_call("/Effects", "org.kde.kwin.Effects.reconfigureEffect", Some(EFFECT_ID)) {
        eprintln!("lunar-desktop: wrote the glass strengths but KWin did not accept reconfigureEffect({EFFECT_ID})");
    }
}

fn dbus_call(path: &str, method: &str, argument: Option<&str>) -> bool {
    let mut command = Command::new("dbus-send");
    command
        .args(["--session", "--dest=org.kde.KWin", "--type=method_call", path, method])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(argument) = argument {
        command.arg(format!("string:{argument}"));
    }
    command.status().is_ok_and(|status| status.success())
}

/// Applies the strength from a background thread.
pub fn apply_in_background(percent: f64) {
    std::thread::spawn(move || apply(percent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_maps_onto_plugin_range() {
        assert_eq!(strength_for(0.0), MIN_STRENGTH);
        assert_eq!(strength_for(100.0), MAX_STRENGTH);
        assert_eq!(strength_for(-5.0), MIN_STRENGTH);
        assert_eq!(strength_for(500.0), MAX_STRENGTH);
        // Halfway, rounded to the nearest whole strength.
        assert_eq!(strength_for(50.0), 4);
        // Values always stay inside the range and never go backwards.
        let mut last = MIN_STRENGTH;
        for percent in 0..=100 {
            let strength = strength_for(f64::from(percent));
            assert!((MIN_STRENGTH..=MAX_STRENGTH).contains(&strength));
            assert!(strength >= last);
            last = strength;
        }
    }
}
