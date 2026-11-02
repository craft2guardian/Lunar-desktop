//! sonoma-desktop: a minimal macOS Sonoma-inspired desktop shell for KDE Plasma.

mod about;
mod animation;
mod config;
mod context_menu;
mod desktop;
mod dock;
mod glass;
mod kde_shell;
mod kwin_windows;
mod menubar;
mod preferences;
mod settings;
mod style;
mod wallpaper;

use gtk::glib;
use gtk::prelude::*;

const APP_ID: &str = "dev.sonoma.Desktop";

fn main() -> glib::ExitCode {
    // GTK otherwise draws its own default titlebar on some Wayland sessions.
    // KDE provides server-side decorations, so let KWin render window borders.
    std::env::set_var("GTK_CSD", "0");
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(activate);
    app.run()
}

fn activate(app: &gtk::Application) {
    crate::animation::disable_by_default();
    if !kde_shell::is_supported() {
        fail(
            "this session cannot host the KDE desktop shell: run it in a KDE Plasma \
             Wayland session that provides the org_kde_plasma_shell protocol.",
        );
    }

    if let Err(message) = desktop::Desktop::start(app) {
        fail(&message);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("sonoma-desktop: {message}");
    std::process::exit(1);
}
