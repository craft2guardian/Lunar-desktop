//! Standalone CLI diagnostics for the production KWin window tracker.
//! This binary never creates or presents GTK widgets.

#[path = "../config.rs"]
#[allow(dead_code)]
mod config;
/// `kwin_windows` recognises the shell's own Settings window by its title.
mod settings {
    pub const WINDOW_TITLE: &str = "System Settings";
}
#[path = "../kwin_windows.rs"]
mod kwin_windows;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gtk::glib;
use kwin_windows::{AppWindow, KWinWindows, WindowSnapshot};

fn main() {
    println!("LUNAR DOCK TRACKER DIAGNOSTICS\n");
    let tracker = match KWinWindows::start() {
        Ok(tracker) => tracker,
        Err(error) => {
            println!("KWin Connection: FAILED\nWindow Enumeration: FAILED\nWindow Event Listener: FAILED");
            eprintln!("KWin tracker startup failed: {error}");
            std::process::exit(1);
        }
    };
    println!("KWin Connection: CONNECTED");
    println!("Window Enumeration: WAITING FOR INITIAL SNAPSHOT");
    println!("Window Event Listener: ACTIVE");

    let entries = config::load_dock();
    if entries.is_empty() {
        eprintln!("Dock configuration contains no usable application entries.");
    }
    println!("Dock entries loaded: {}", entries.len());
    println!("Commands: list | windows | unmatched | watch | quit\n");

    let (sender, receiver) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let main_loop = glib::MainLoop::new(None, false);
    let loop_for_tick = main_loop.clone();
    let entries = std::rc::Rc::new(entries);
    let tick_tracker = tracker.clone();
    let mut watch = false;
    let mut seen_revision = 0_u64;
    let mut previous_windows = HashMap::<String, AppWindow>::new();
    let started = Instant::now();
    let mut enumeration_timeout_reported = false;

    glib::timeout_add_local(Duration::from_millis(75), move || {
        let snapshot = tick_tracker.snapshot();
        if snapshot.revision > seen_revision {
            if seen_revision == 0 {
                println!(
                    "Window Enumeration: WORKING ({} windows)",
                    snapshot.windows.len()
                );
            }
            if watch {
                print_window_changes(&snapshot, &previous_windows);
                print_running_applications(&entries, &snapshot);
            }
            previous_windows = snapshot
                .windows
                .iter()
                .cloned()
                .map(|window| (window.window_id.clone(), window))
                .collect();
            seen_revision = snapshot.revision;
        } else if seen_revision == 0
            && !enumeration_timeout_reported
            && started.elapsed() >= Duration::from_secs(5)
        {
            eprintln!("Window Enumeration: FAILED (KWin started the tracker, but no initial snapshot arrived within 5 seconds)");
            enumeration_timeout_reported = true;
        }

        while let Ok(line) = receiver.try_recv() {
            match line.trim().to_ascii_lowercase().as_str() {
                "list" => print_running_applications(&entries, &snapshot),
                "windows" => print_windows(&entries, &snapshot),
                "unmatched" => print_unmatched_windows(&entries, &snapshot),
                "watch" => {
                    watch = !watch;
                    println!(
                        "Window event watch {}.",
                        if watch { "enabled" } else { "paused" }
                    );
                }
                "quit" | "exit" => {
                    println!("Stopping tracker diagnostics.");
                    loop_for_tick.quit();
                    return glib::ControlFlow::Break;
                }
                "" => {}
                other => println!(
                    "Unknown command {other:?}; use list, windows, unmatched, watch, or quit."
                ),
            }
            print!("> ");
            let _ = io::stdout().flush();
        }
        glib::ControlFlow::Continue
    });

    print!("> ");
    let _ = io::stdout().flush();
    main_loop.run();
    drop(tracker);
}

fn aliases_for_entry(command: &str) -> Vec<String> {
    let Ok(argv) = glib::shell_parse_argv(command) else {
        return Vec::new();
    };
    let Some(program) = argv.first().and_then(|arg| {
        Path::new(arg.as_os_str())
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }) else {
        return Vec::new();
    };
    kwin_windows::aliases_for_program(&program)
}

fn print_running_applications(entries: &[config::Entry], snapshot: &WindowSnapshot) {
    println!("\nRUNNING APPLICATIONS");
    let mut found = false;
    for entry in entries {
        let aliases = aliases_for_entry(&entry.command);
        let (is_running, _, _) = kwin_windows::matching_window_state(&aliases, snapshot);
        if !is_running {
            continue;
        }
        let groups = kwin_windows::matching_window_groups(&aliases, snapshot);
        for (identifier, windows) in groups {
            found = true;
            let focused = windows.iter().any(|window| window.focused);
            println!(
                "\n[RUNNING] {}\nIdentifier: {}\nWindows: {}\nFocused: {}",
                entry.name,
                identifier,
                windows.len(),
                if focused { "Yes" } else { "No" }
            );
        }
    }
    if !found {
        println!("\nNo running applications matched the configured Dock entries.");
    }
    println!();
}

fn print_windows(entries: &[config::Entry], snapshot: &WindowSnapshot) {
    println!("\nDETECTED WINDOWS ({})", snapshot.windows.len());
    for window in &snapshot.windows {
        let matches = matching_entry_names(entries, window);
        println!(
            "id={:?} identity={:?} app_id={:?} resource_class={:?} resource_name={:?} title={:?} pid={} minimized={} focused={} special={} trackable={} dock_matches={matches:?}",
            window.window_id,
            window.resolved_identity(),
            window.desktop_file,
            window.resource_class,
            window.resource_name,
            window.title,
            window.pid,
            window.minimized,
            window.focused,
            window.special,
            window.is_trackable_app()
        );
    }
    println!();
}

fn print_unmatched_windows(entries: &[config::Entry], snapshot: &WindowSnapshot) {
    println!("\nUNMATCHED WINDOWS");
    let unmatched: Vec<_> = snapshot
        .windows
        .iter()
        .filter(|window| {
            window.is_trackable_app() && matching_entry_names(entries, window).is_empty()
        })
        .collect();
    if unmatched.is_empty() {
        println!("No unmatched application windows.");
    }
    for window in unmatched {
        println!(
            "id={:?} identity={:?} app_id={:?} resource_class={:?} resource_name={:?} title={:?} minimized={} focused={}",
            window.window_id,
            window.resolved_identity(),
            window.desktop_file,
            window.resource_class,
            window.resource_name,
            window.title,
            window.minimized,
            window.focused
        );
    }
    println!();
}

fn matching_entry_names(entries: &[config::Entry], window: &AppWindow) -> Vec<String> {
    if !window.is_trackable_app() {
        return Vec::new();
    }
    entries
        .iter()
        .filter(|entry| window.matches(&aliases_for_entry(&entry.command)))
        .map(|entry| entry.name.clone())
        .collect()
}

fn print_window_changes(snapshot: &WindowSnapshot, previous: &HashMap<String, AppWindow>) {
    println!("\nWINDOW EVENT: {}", snapshot.event);
    let current: HashMap<_, _> = snapshot
        .windows
        .iter()
        .map(|window| (window.window_id.as_str(), window))
        .collect();
    for window in &snapshot.windows {
        match previous.get(&window.window_id) {
            None => println!(
                "  ADDED {} ({})",
                window.window_id,
                window.resolved_identity()
            ),
            Some(old) if old != window => println!(
                "  CHANGED {} ({}) minimized={} focused={} title={:?}",
                window.window_id,
                window.resolved_identity(),
                window.minimized,
                window.focused,
                window.title
            ),
            Some(_) => {}
        }
    }
    for id in previous
        .keys()
        .filter(|id| !current.contains_key(id.as_str()))
    {
        println!("  REMOVED {id}");
    }
    if snapshot.windows.is_empty() {
        println!("  No windows currently reported by KWin.");
    }
    println!();
}
