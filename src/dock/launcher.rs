//! Process launching, command parsing, and `.desktop` file helpers for the Dock.

use std::path::Path;
use std::process::Command;
use std::thread;

use gtk::prelude::*;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::config::Entry;
use crate::kwin_windows;

/// Spawns `program` fully detached from the Dock: in its own session (so it
/// is not part of the Dock's process group, terminal or job and survives the
/// Dock exiting or being killed), with stdin/stdout/stderr closed, and reaped
/// by a helper thread so no zombie is left behind.
pub fn spawn_detached(
    program: &std::ffi::OsStr,
    args: &[&std::ffi::OsStr],
) -> std::io::Result<()> {
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
pub(crate) fn launch(command: &str) {
    let argv = match glib::shell_parse_argv(command) {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => return,
        Err(e) => {
            eprintln!("lunar-desktop: bad dock command {command:?}: {e}");
            return;
        }
    };
    let args: Vec<&std::ffi::OsStr> = argv[1..].iter().map(|a| a.as_os_str()).collect();
    if let Err(e) = spawn_detached(argv[0].as_os_str(), &args) {
        eprintln!("lunar-desktop: could not launch {command:?}: {e}");
    }
}

/// Finds the `.desktop` file of an installed app in the XDG data directories.
pub fn desktop_file_for(app: &gio::AppInfo) -> Option<std::path::PathBuf> {
    let id = app.id()?;
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    dirs.into_iter()
        .map(|dir| dir.join("applications").join(id.as_str()))
        .find(|path| path.is_file())
}

/// Launches an installed desktop entry through `gio launch` in a detached
/// process. Returns false if that was not possible.
pub(crate) fn launch_app_detached(app: &gio::AppInfo) -> bool {
    let Some(path) = desktop_file_for(app) else {
        return false;
    };
    spawn_detached(
        std::ffi::OsStr::new("gio"),
        &[std::ffi::OsStr::new("launch"), path.as_os_str()],
    )
    .is_ok()
}

pub(crate) fn installed_app_for(program: &str) -> Option<gio::AppInfo> {
    kwin_windows::installed_app_for_program(program)
        .or_else(|| kwin_windows::installed_app_for_identity(program))
}

/// The program name of a command, e.g. "firefox" for "/usr/bin/firefox --new-window".
pub(crate) fn program_name(command: &str) -> Option<String> {
    let argv = glib::shell_parse_argv(command).ok()?;
    let first = argv.first()?;
    Some(
        Path::new(first)
            .file_name()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// The shell's own application, for presenting the System Settings window.
pub(crate) fn shell_application() -> Option<gtk::Application> {
    gio::Application::default().and_then(|app| app.downcast::<gtk::Application>().ok())
}

/// System Settings runs inside this process, so its Dock tile acts on the
/// window directly instead of going through KWin or a launcher.
pub(crate) fn settings_window_action(action: &str) {
    let Some(app) = shell_application() else {
        return;
    };
    match action {
        "activate" => crate::settings::present(&app),
        // The Dock icon was clicked while minimised windows live in their own
        // Dock tiles: raise the window only if it is not minimised.
        "activate-visible" => {
            let minimized = crate::settings::find_window(&app)
                .and_then(|window| window.surface())
                .and_then(|surface| surface.downcast::<gdk::Toplevel>().ok())
                .is_some_and(|toplevel| toplevel.state().contains(gdk::ToplevelState::MINIMIZED));
            if !minimized {
                crate::settings::present(&app);
            }
        }
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

pub(crate) fn launch_entry(entry: &Entry) {
    if entry.command == crate::launchpad::COMMAND {
        crate::launchpad::toggle();
        return;
    }
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
                        "lunar-desktop: desktop entry for {} failed to launch: {error}",
                        entry.name
                    );
                }
                return;
            }
        }
    }
    launch(&entry.command);
}
