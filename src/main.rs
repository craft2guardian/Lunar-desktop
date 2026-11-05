//! lunar-desktop: a minimal macOS Sonoma-inspired desktop shell for KDE Plasma.

mod about;
mod animation;
mod config;
mod context_menu;
mod control_center;
mod desktop;
mod dock;
mod glass;
mod glass_config;
mod glass_preview;
mod kde_shell;
mod kwin_windows;
mod liquid_glass;
mod magnify;
mod menubar;
mod preferences;
mod settings;
mod style;
mod thumbnail;
mod wallpaper;

use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "dev.lunar.Desktop";

/// Keeps glibc from hoarding freed memory. Window screenshots are tens of MB
/// and are taken on short-lived threads; with default settings glibc keeps
/// those blocks (and a separate arena per thread) resident long after they are
/// freed, which shows up as hundreds of MB of RSS for an idle shell.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn limit_malloc_retention() {
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    const M_TRIM_THRESHOLD: i32 = -1;
    const M_MMAP_THRESHOLD: i32 = -3;
    const M_ARENA_MAX: i32 = -8;
    // SAFETY: mallopt only adjusts allocator tunables; called before any threads start.
    unsafe {
        mallopt(M_ARENA_MAX, 2);
        // A fixed threshold stops glibc raising it dynamically; blocks of 256 KiB
        // and up are mmap'd and returned to the OS the moment they are freed.
        mallopt(M_MMAP_THRESHOLD, 256 * 1024);
        mallopt(M_TRIM_THRESHOLD, 1024 * 1024);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn limit_malloc_retention() {}

fn main() -> glib::ExitCode {
    // GTK otherwise draws its own default titlebar on some Wayland sessions.
    // KDE provides server-side decorations, so let KWin render window borders.
    std::env::set_var("GTK_CSD", "0");
    limit_malloc_retention();
    // Before anything reads the settings folder.
    crate::preferences::migrate_old_config();
    // GTK 4.16+ defaults to Vulkan, which reserves far more memory per surface
    // than the GL renderer. Respect an explicit choice from the user.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // GTK 4.20 renamed the "ngl" renderer to "gl"; older versions only know "ngl".
        let name = if gtk::minor_version() >= 20 { "gl" } else { "ngl" };
        std::env::set_var("GSK_RENDERER", name);
    }
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(activate);
    app.run()
}

fn activate(app: &gtk::Application) {
    crate::animation::disable_by_default();
    // Developer preview of the Liquid Glass surfaces (see glass_preview.rs).
    if let Ok(scene) = std::env::var("LUNAR_GLASS_PREVIEW") {
        glass_preview::run(app, &scene);
        return;
    }
    if !kde_shell::is_supported() {
        fail(
            "this session cannot host the desktop shell: run it under KWin on Wayland, \
             which provides org_kde_plasma_shell.",
        );
    }

    if let Err(message) = desktop::Desktop::start(app) {
        fail(&message);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("lunar-desktop: {message}");
    std::process::exit(1);
}
