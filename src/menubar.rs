//! The menu bar: one thin, full-width `top`-layer surface per output, flush
//! with the top edge. KDE sessions show the KDE logo at the left.

use crate::context_menu;
use crate::kde_shell::{self, Role};
use crate::kwin_windows::{self, KWinWindows, WindowSnapshot};
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::rc::Rc;
use std::time::Duration;

use crate::glass::GlassStrip;

/// Bar height in logical pixels (macOS uses 24 pt; 28 matches the 26-30 px brief).
pub const HEIGHT: i32 = 28;

fn is_kde_session() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    desktop
        .split([':', ';'])
        .any(|name| name.contains("kde") || name.contains("plasma"))
        || std::env::var_os("KDE_SESSION_VERSION").is_some()
}

/// One of the shell's own popup surfaces (menus, popovers): KWin reports them
/// as windows of "lunar-desktop" with no title and no application id, and
/// focuses them while a menu is open.
fn is_own_popup(window: &crate::kwin_windows::AppWindow) -> bool {
    window.desktop_file.trim().is_empty()
        && window.title.trim().is_empty()
        && (window.resource_name == "lunar-desktop" || window.resource_class == "lunar-desktop")
}

fn is_own_window(window: &crate::kwin_windows::AppWindow) -> bool {
    window.resolved_identity().starts_with("dev.lunar")
        || window.resource_name == "lunar-desktop"
        || window.resource_class == "lunar-desktop"
}

fn active_application(snapshot: &WindowSnapshot) -> (String, Option<String>) {
    let Some(window) = snapshot.windows.iter().find(|window| window.focused) else {
        return ("Finder".to_string(), None);
    };

    if is_own_window(window) {
        if window.title.contains("System Settings") {
            return ("System Settings".to_string(), None);
        }
        return ("Finder".to_string(), None);
    }

    let name = kwin_windows::installed_app_for_identity(&window.resolved_identity())
        .map(|app| app.display_name().to_string())
        .unwrap_or_else(|| {
            let identifier = if !window.resource_name.trim().is_empty() {
                &window.resource_name
            } else if !window.resource_class.trim().is_empty() {
                &window.resource_class
            } else {
                &window.title
            };
            identifier
                .rsplit(['.', '/', '-'])
                .next()
                .filter(|name| !name.is_empty())
                .map(|name| {
                    let mut chars = name.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().collect::<String>() + chars.as_str()
                    })
                })
                .unwrap_or_else(|| "Finder".to_string())
        });

    let target = [&window.resource_name, &window.resource_class]
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| (!window.desktop_file.trim().is_empty()).then(|| window.resolved_identity()));
    (name, target)
}

/// The text macOS shows in the menu bar clock, e.g. "Sun 1 Nov 3:19 PM".
fn clock_text() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%a %-d %b %-I:%M %p").ok())
        .map(|text| text.to_string())
        .unwrap_or_default()
}

#[derive(Clone)]
struct WifiNetwork {
    ssid: String,
    signal: u8,
    active: bool,
    secured: bool,
}

struct WifiState {
    available: bool,
    enabled: bool,
    networks: Vec<WifiNetwork>,
}

/// Splits one line of `nmcli -t` output (fields separated by `:`, with `\:`
/// and `\\` as escapes).
pub(crate) fn split_terse(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    fields.last_mut().unwrap().push(next);
                }
            }
            ':' => fields.push(String::new()),
            _ => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

/// Reads the Wi-Fi radio state and nearby networks from NetworkManager's
/// cached scan results (instant; see `rescan_wifi` for fresh ones).
/// Blocking: run it off the main thread.
fn read_wifi_state() -> WifiState {
    let unavailable = WifiState {
        available: false,
        enabled: false,
        networks: Vec::new(),
    };
    let Ok(radio) = std::process::Command::new("nmcli")
        .args(["-t", "-f", "WIFI", "radio"])
        .output()
    else {
        return unavailable;
    };
    if !radio.status.success() {
        return unavailable;
    }
    let enabled = String::from_utf8_lossy(&radio.stdout).trim() == "enabled";
    let mut networks: Vec<WifiNetwork> = Vec::new();
    if enabled {
        if let Ok(list) = std::process::Command::new("nmcli")
            .args([
                "-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "no",
            ])
            .output()
        {
            for line in String::from_utf8_lossy(&list.stdout).lines() {
                let fields = split_terse(line);
                if fields.len() < 3 || fields[1].trim().is_empty() {
                    continue;
                }
                let network = WifiNetwork {
                    ssid: fields[1].clone(),
                    signal: fields[2].trim().parse().unwrap_or(0),
                    active: fields[0].trim() == "*",
                    secured: fields
                        .get(3)
                        .is_some_and(|security| !matches!(security.trim(), "" | "--")),
                };
                match networks.iter_mut().find(|known| known.ssid == network.ssid) {
                    Some(known) => {
                        known.active |= network.active;
                        known.secured |= network.secured;
                        known.signal = known.signal.max(network.signal);
                    }
                    None => networks.push(network),
                }
            }
        }
        networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
        networks.truncate(50);
    }
    WifiState {
        available: true,
        enabled,
        networks,
    }
}

/// Asks NetworkManager for a fresh scan. Takes a few seconds.
fn rescan_wifi() {
    let _ = std::process::Command::new("nmcli")
        .args(["device", "wifi", "rescan"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn signal_bars(signal: u8) -> &'static str {
    match signal {
        0..=24 => "\u{2582}",
        25..=49 => "\u{2582}\u{2584}",
        50..=74 => "\u{2582}\u{2584}\u{2586}",
        _ => "\u{2582}\u{2584}\u{2586}\u{2588}",
    }
}

/// Opens KDE's network settings page.
pub(crate) fn open_network_settings() {
    use std::ffi::OsStr;
    for (program, args) in [
        ("kcmshell6", "kcm_networkmanagement"),
        ("kcmshell5", "kcm_networkmanagement"),
        ("systemsettings", "kcm_networkmanagement"),
    ] {
        if crate::dock::spawn_detached(OsStr::new(program), &[OsStr::new(args)]).is_ok() {
            return;
        }
    }
    eprintln!("lunar-desktop: could not open the network settings");
}

/// Joins a network with NetworkManager. Networks that need a new password
/// can't be joined this way, so the network settings open instead.
fn connect_to_network(ssid: String) {
    std::thread::spawn(move || {
        let joined = std::process::Command::new("nmcli")
            .args(["device", "wifi", "connect", &ssid])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !joined {
            open_network_settings();
        }
    });
}

/// The theme's Wi-Fi glyph, if it has one.
fn wifi_icon_name() -> Option<&'static str> {
    let theme = gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display))?;
    [
        "network-wireless-signal-excellent-symbolic",
        "network-wireless-symbolic",
        "network-wireless",
    ]
    .into_iter()
    .find(|name| theme.has_icon(name))
}

/// The small padlock shown beside secured networks.
fn lock_icon() -> gtk::Widget {
    let theme = gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display));
    let name = theme.and_then(|theme| {
        [
            "changes-prevent-symbolic",
            "system-lock-screen-symbolic",
            "object-locked-symbolic",
        ]
        .into_iter()
        .find(|name| theme.has_icon(name))
    });
    let widget: gtk::Widget = match name {
        Some(name) => {
            let image = gtk::Image::from_icon_name(name);
            image.set_pixel_size(13);
            image.upcast()
        }
        None => gtk::Label::new(Some("\u{1F512}")).upcast(),
    };
    widget.add_css_class("wifi-lock");
    widget.set_valign(gtk::Align::Center);
    widget
}

/// One clickable network row. The `featured` row (the connected network) gets
/// the blue Wi-Fi badge and no signal bars; the others show a tick or bars.
fn network_button(
    network: &WifiNetwork,
    main: &glib::WeakRef<gtk::Popover>,
    featured: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, if featured { 10 } else { 8 });

    if featured {
        let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        badge.add_css_class("wifi-badge");
        badge.set_size_request(28, 28);
        badge.set_hexpand(false);
        badge.set_halign(gtk::Align::Start);
        badge.set_valign(gtk::Align::Center);
        let glyph: gtk::Widget = match wifi_icon_name() {
            Some(name) => {
                let image = gtk::Image::from_icon_name(name);
                image.set_pixel_size(15);
                image.upcast()
            }
            None => gtk::Label::new(Some("\u{2582}\u{2584}\u{2586}")).upcast(),
        };
        glyph.add_css_class("wifi-badge-icon");
        glyph.set_hexpand(true);
        glyph.set_halign(gtk::Align::Center);
        glyph.set_valign(gtk::Align::Center);
        badge.append(&glyph);
        row.append(&badge);
    } else {
        let check = gtk::Label::new(Some(if network.active { "\u{2713}" } else { "" }));
        check.set_width_chars(2);
        row.append(&check);
    }

    let name = gtk::Label::new(Some(&network.ssid));
    name.set_halign(gtk::Align::Start);
    name.set_hexpand(true);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(24);
    row.append(&name);

    if network.secured {
        row.append(&lock_icon());
    }
    if !featured {
        row.append(&gtk::Label::new(Some(signal_bars(network.signal))));
    }

    button.set_child(Some(&row));
    button.set_halign(gtk::Align::Fill);
    button.add_css_class("lunar-menu-item");
    if featured {
        button.add_css_class("wifi-current-item");
    }

    let main = main.clone();
    let ssid = network.ssid.clone();
    let active = network.active;
    button.connect_clicked(move |_| {
        if let Some(popover) = main.upgrade() {
            popover.popdown();
        }
        if !active {
            connect_to_network(ssid.clone());
        }
    });
    button
}

/// The Wi-Fi icon at the right end of the bar, laid out like macOS: the
/// on/off switch, the connected ("Preferred") network with a blue badge, an
/// "Other Networks" submenu, and "Wi-Fi Settings…", via NetworkManager.
fn wifi_menu() -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");
    menu.add_css_class("wifi-menu");
    menu.set_size_request(250, -1);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_start(6);
    header.set_margin_end(6);
    header.set_margin_top(2);
    header.set_margin_bottom(2);
    let title = gtk::Label::new(Some("Wi-Fi"));
    title.add_css_class("wifi-title");
    title.set_halign(gtk::Align::Start);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    header.append(&title);
    header.append(&switch);
    menu.append(&header);
    append_separator(&menu);

    // "Preferred Network" and the connected network. Always the same height,
    // so the popover does not resize when the connection changes.
    let current = gtk::Box::new(gtk::Orientation::Vertical, 0);
    current.set_size_request(-1, 54);
    menu.append(&current);
    append_separator(&menu);

    // "Other Networks  v" expands a list of the remaining networks in place.
    let others_button = gtk::Button::new();
    let others_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let others_text = gtk::Label::new(Some("Other Networks"));
    others_text.set_halign(gtk::Align::Start);
    others_text.set_hexpand(true);
    others_text.set_xalign(0.0);
    let others_arrow = gtk::Label::new(Some("\u{203A}"));
    others_arrow.add_css_class("wifi-chevron");
    others_row.append(&others_text);
    others_row.append(&others_arrow);
    others_button.set_child(Some(&others_row));
    others_button.set_halign(gtk::Align::Fill);
    others_button.add_css_class("lunar-menu-item");
    others_button.add_css_class("wifi-heading-item");
    menu.append(&others_button);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("wifi-dropdown");
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    scroller.set_min_content_height(0);
    scroller.set_max_content_height(150);
    scroller.set_child(Some(&list));
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(120);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(&scroller));
    menu.append(&revealer);

    append_separator(&menu);
    append_menu_item(
        &menu,
        &popover,
        "Wi-Fi Settings\u{2026}",
        Some(Box::new(open_network_settings)),
    );

    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    popover.add_css_class("lunar-blurred-popover");
    // Blur just the menu body, and re-fit it when the dropdown opens or closes.
    let refresh_blur = context_menu::attach_blur_region(
        &popover,
        menu.upcast_ref(),
        crate::liquid_glass::Material::menu().blur_corner(),
    );
    {
        let revealer = revealer.clone();
        let arrow = others_arrow.clone();
        others_button.connect_clicked(move |_| {
            let open = !revealer.reveals_child();
            revealer.set_reveal_child(open);
            arrow.set_text(if open { "\u{02C5}" } else { "\u{203A}" });
            refresh_blur();
        });
    }
    {
        // Start collapsed every time the menu opens.
        let revealer = revealer.clone();
        let arrow = others_arrow.clone();
        popover.connect_closed(move |_| {
            revealer.set_reveal_child(false);
            arrow.set_text("\u{203A}");
        });
    }

    // True while the switch is being set from NetworkManager's state, so the
    // change is not mistaken for the user flipping it.
    let updating = Rc::new(std::cell::Cell::new(false));
    // False until the first fresh scan has finished.
    let scanned = Rc::new(std::cell::Cell::new(false));

    let refresh: Rc<dyn Fn()> = {
        let scanned = Rc::clone(&scanned);
        let list = list.downgrade();
        let current = current.downgrade();
        let others_button = others_button.downgrade();
        let switch = switch.downgrade();
        let popover = popover.downgrade();
        let updating = Rc::clone(&updating);
        Rc::new(move || {
            let list = list.clone();
            let current = current.clone();
            let others_button = others_button.clone();
            let switch = switch.clone();
            let popover = popover.clone();
            let updating = Rc::clone(&updating);
            let scanned = Rc::clone(&scanned);
            glib::spawn_future_local(async move {
                let Ok(state) = gtk::gio::spawn_blocking(read_wifi_state).await else {
                    return;
                };
                let (Some(list), Some(current), Some(others_button), Some(switch), Some(popover)) = (
                    list.upgrade(),
                    current.upgrade(),
                    others_button.upgrade(),
                    switch.upgrade(),
                    popover.upgrade(),
                ) else {
                    return;
                };
                updating.set(true);
                switch.set_active(state.enabled);
                switch.set_sensitive(state.available);
                updating.set(false);
                others_button.set_sensitive(state.available && state.enabled);
                let main = popover.downgrade();

                // Connected network under "Preferred Network".
                while let Some(child) = current.first_child() {
                    current.remove(&child);
                }
                match state.networks.iter().find(|network| network.active) {
                    Some(network) => {
                        let heading = gtk::Label::new(Some("Preferred Network"));
                        heading.add_css_class("wifi-section-title");
                        heading.set_halign(gtk::Align::Start);
                        heading.set_xalign(0.0);
                        current.append(&heading);
                        current.append(&network_button(network, &main, true));
                    }
                    None => {
                        let text = if !state.available {
                            "Wi-Fi is unavailable"
                        } else if !state.enabled {
                            "Wi-Fi is turned off"
                        } else {
                            "Not connected"
                        };
                        let status = gtk::Label::new(Some(text));
                        status.add_css_class("wifi-status");
                        status.set_halign(gtk::Align::Start);
                        status.set_valign(gtk::Align::Center);
                        status.set_vexpand(true);
                        status.set_xalign(0.0);
                        current.append(&status);
                    }
                }

                // Everything else goes in the "Other Networks" submenu.
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
                let others: Vec<&WifiNetwork> = state
                    .networks
                    .iter()
                    .filter(|network| !network.active)
                    .collect();
                if state.enabled && others.is_empty() {
                    let text = if scanned.get() {
                        "No other networks found"
                    } else {
                        "Looking for networks\u{2026}"
                    };
                    append_menu_item(&list, &popover, text, None);
                }
                for network in others {
                    list.append(&network_button(network, &main, false));
                }
            });
        })
    };

    {
        let refresh = Rc::clone(&refresh);
        let updating = Rc::clone(&updating);
        switch.connect_active_notify(move |switch| {
            if updating.get() {
                return;
            }
            let argument = if switch.is_active() { "on" } else { "off" };
            let refresh = Rc::clone(&refresh);
            glib::spawn_future_local(async move {
                let _ = gtk::gio::spawn_blocking(move || {
                    std::process::Command::new("nmcli")
                        .args(["radio", "wifi", argument])
                        .status()
                })
                .await;
                glib::timeout_future(Duration::from_millis(1500)).await;
                refresh();
            });
        });
    }
    {
        // Show what NetworkManager already knows straight away, then scan
        // and update the list when fresh results arrive.
        let refresh = Rc::clone(&refresh);
        let scanned = Rc::clone(&scanned);
        popover.connect_show(move |_| {
            refresh();
            let refresh = Rc::clone(&refresh);
            let scanned = Rc::clone(&scanned);
            glib::spawn_future_local(async move {
                let _ = gtk::gio::spawn_blocking(rescan_wifi).await;
                scanned.set(true);
                refresh();
            });
        });
    }

    let icon: gtk::Widget = match wifi_icon_name() {
        Some(name) => {
            let image = gtk::Image::from_icon_name(name);
            image.set_pixel_size(16);
            image.add_css_class("lunar-menubar-item");
            image.upcast()
        }
        None => {
            let label = gtk::Label::new(Some("Wi-Fi"));
            label.add_css_class("lunar-menubar-item");
            label.upcast()
        }
    };
    let button = gtk::MenuButton::new();
    button.set_size_request(30, HEIGHT);
    button.set_child(Some(&icon));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::End);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);
    button
}

/// The clock at the right-hand end of the bar. It re-checks the time every
/// second but only touches the label when the minute actually changes.
fn clock_label() -> gtk::Label {
    let label = gtk::Label::new(Some(&clock_text()));
    label.add_css_class("lunar-menubar-item");
    label.add_css_class("lunar-menubar-clock");
    label.set_halign(gtk::Align::End);
    label.set_valign(gtk::Align::Center);
    label.set_margin_end(14);
    // Purely informational: never steal clicks from what is beneath it.
    label.set_can_target(false);

    let weak = label.downgrade();
    glib::timeout_add_seconds_local(1, move || {
        let Some(label) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let text = clock_text();
        if label.text() != text {
            label.set_text(&text);
        }
        glib::ControlFlow::Continue
    });
    label
}

fn append_menu_item(
    menu: &gtk::Box,
    popover: &gtk::Popover,
    label: &str,
    action: Option<Box<dyn Fn() + 'static>>,
) {
    let button = gtk::Button::new();
    let text = gtk::Label::new(Some(label));
    text.set_halign(gtk::Align::Start);
    button.set_child(Some(&text));
    button.set_halign(gtk::Align::Fill);
    button.add_css_class("lunar-menu-item");
    if let Some(action) = action {
        let popover = popover.downgrade();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            action();
        });
    } else {
        button.set_sensitive(false);
    }
    menu.append(&button);
}

fn append_separator(menu: &gtk::Box) {
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.add_css_class("lunar-menu-separator");
    menu.append(&separator);
}

fn spawn_first_available(commands: &[&str]) {
    for command in commands {
        match std::process::Command::new(command).spawn() {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!("lunar-desktop: could not start {command}: {error}");
                return;
            }
        }
    }
    eprintln!("lunar-desktop: no system monitor application was found");
}

fn run_system_action(program: &str, argument: &str) {
    if let Err(error) = std::process::Command::new(program).arg(argument).spawn() {
        eprintln!("lunar-desktop: could not run {program} {argument}: {error}");
    }
}

fn run_logout() {
    // In the Lunar session there is no KDE session manager to ask; fall through
    // to logind below.
    if is_kde_session() && std::env::var_os("LUNAR_SESSION").is_none() {
        for command in ["qdbus6", "qdbus"] {
            match std::process::Command::new(command)
                .args(["org.kde.ksmserver", "/KSMServer", "logout", "1", "0", "0"])
                .spawn()
            {
                Ok(_) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    eprintln!("lunar-desktop: could not request KDE logout: {error}");
                    return;
                }
            }
        }
    }
    if let Some(session) = std::env::var_os("XDG_SESSION_ID") {
        if let Err(error) = std::process::Command::new("loginctl")
            .arg("terminate-session")
            .arg(session)
            .spawn()
        {
            eprintln!("lunar-desktop: could not log out: {error}");
        }
    } else {
        eprintln!("lunar-desktop: XDG_SESSION_ID is not set; cannot log out");
    }
}

/// The app whose windows File > New Window should create: the focused
/// application's desktop identity, or `None` for the shell itself (Finder).
fn new_window_identity(snapshot: &WindowSnapshot) -> Option<String> {
    let window = snapshot.windows.iter().find(|window| window.focused)?;
    let identity = window.resolved_identity();
    if is_own_window(window) {
        None
    } else {
        Some(identity)
    }
}

/// Identities (lower-case desktop IDs) of terminal emulators.
const TERMINALS: &[&str] = &[
    "konsole",
    "terminal",
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "ghostty",
    "xterm",
    "tilix",
    "terminator",
];

/// Wording of the first File item, as on macOS: Finder and Terminal name
/// themselves; everything else says plain "New Window".
fn new_window_title(identity: Option<&str>) -> &'static str {
    match identity {
        None => "New Finder Window",
        Some(identity) if TERMINALS.iter().any(|name| identity.contains(name)) => {
            "New Terminal Window"
        }
        Some(_) => "New Window",
    }
}

/// Opens a new window: launches the app's desktop entry again; if that is not
/// possible, starts the program directly. For Finder (no app focused) it opens
/// a file manager.
fn new_window(identity: Option<String>) {
    eprintln!("lunar-desktop: File > New Window for {identity:?}");
    let app_info = match &identity {
        Some(identity) => kwin_windows::installed_app_for_identity(identity),
        None => gtk::gio::AppInfo::default_for_type("inode/directory", false),
    };
    match app_info {
        Some(app_info) => match app_info.launch(&[], None::<&gtk::gio::AppLaunchContext>) {
            Ok(()) => return,
            Err(error) => eprintln!("lunar-desktop: desktop-entry launch failed: {error}"),
        },
        None => eprintln!("lunar-desktop: no desktop entry found, trying the program directly"),
    }

    let candidates: Vec<String> = match &identity {
        Some(identity) => vec![identity.rsplit('.').next().unwrap_or(identity).to_string()],
        None => ["dolphin", "nautilus", "thunar", "pcmanfm"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
    };
    for program in candidates {
        match std::process::Command::new(&program).spawn() {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!("lunar-desktop: could not start {program}: {error}");
                return;
            }
        }
    }
    eprintln!("lunar-desktop: no program could be started for New Window");
}

/// The "File" title next to the app name, with its menu. Also returns the
/// label of the first item so it can be renamed when the focused app changes.
fn file_menu(
    target: &std::rc::Rc<std::cell::RefCell<Option<String>>>,
) -> (gtk::MenuButton, gtk::Label) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    // Autohide (the default) closes the menu when you click anywhere else,
    // exactly like the Apple and app-name menus.
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");

    let item_label = gtk::Label::new(Some(new_window_title(None)));
    item_label.set_halign(gtk::Align::Start);
    let item = gtk::Button::new();
    item.set_child(Some(&item_label));
    item.set_halign(gtk::Align::Fill);
    item.add_css_class("lunar-menu-item");
    let target = std::rc::Rc::clone(target);
    let popover_weak = popover.downgrade();
    item.connect_clicked(move |_| {
        if let Some(popover) = popover_weak.upgrade() {
            popover.popdown();
        }
        new_window(target.borrow().clone());
    });
    menu.append(&item);
    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&popover);

    let title = gtk::Label::new(Some("File"));
    title.add_css_class("lunar-menubar-item");
    let button = gtk::MenuButton::new();
    button.set_size_request(45, HEIGHT);
    button.set_child(Some(&title));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    // Same classes as the working app-name button, plus the File-specific ones.
    button.add_css_class("lunar-active-app-button");
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);

    // Left-align the menu text under "File" each time it opens, the same way
    // as the app-name menu.
    let button_weak = button.downgrade();
    let title_weak = title.downgrade();
    popover.connect_show(move |popover| {
        if let (Some(button), Some(title)) = (button_weak.upgrade(), title_weak.upgrade()) {
            align_active_menu(&button, &title, popover);
        }
    });

    // Diagnostics: does the menu map and close?
    popover.connect_map(|popover| {
        eprintln!(
            "lunar-desktop: File menu opened (child size {}x{})",
            popover.child().map_or(0, |child| child.width()),
            popover.child().map_or(0, |child| child.height())
        );
    });
    popover.connect_unmap(|_| eprintln!("lunar-desktop: File menu unmapped"));
    popover.connect_closed(|_| eprintln!("lunar-desktop: File menu closed signal"));
    (button, item_label)
}

/// Sends Ctrl+`letter` (Ctrl+Shift+`letter` when `shift`) to the focused application. The menu bar never takes
/// keyboard focus, so the keystroke lands in the app the user was working in.
/// Tries the available synthetic-input tools in turn (the first that works
/// wins): ydotool (uinput, works on Wayland), wtype, then xdotool (XWayland).
fn send_ctrl_shortcut(letter: &'static str, evdev_code: u32, shift: bool) {
    std::thread::spawn(move || {
        // Let the menu finish closing before the keystroke goes out.
        std::thread::sleep(Duration::from_millis(120));
        let code = evdev_code.to_string();
        // ydotool takes raw evdev codes: 29 = Ctrl, 42 = Shift.
        let mut ydotool: Vec<String> = vec!["key".into(), "29:1".into()];
        if shift {
            ydotool.push("42:1".into());
        }
        ydotool.push(format!("{code}:1"));
        ydotool.push(format!("{code}:0"));
        if shift {
            ydotool.push("42:0".into());
        }
        ydotool.push("29:0".into());
        let mut wtype: Vec<String> = vec!["-M".into(), "ctrl".into()];
        if shift {
            wtype.extend(["-M".into(), "shift".into()]);
        }
        wtype.push(letter.into());
        if shift {
            wtype.extend(["-m".into(), "shift".into()]);
        }
        wtype.extend(["-m".into(), "ctrl".into()]);
        let xdotool_keys = if shift {
            format!("ctrl+shift+{letter}")
        } else {
            format!("ctrl+{letter}")
        };
        let attempts: Vec<(&str, Vec<String>)> = vec![
            ("ydotool", ydotool),
            ("wtype", wtype),
            (
                "xdotool",
                vec!["key".into(), "--clearmodifiers".into(), xdotool_keys],
            ),
        ];
        for (program, args) in attempts {
            match std::process::Command::new(program)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(status) if status.success() => return,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    eprintln!("lunar-desktop: could not run {program}: {error}");
                    return;
                }
            }
        }
        eprintln!(
            "lunar-desktop: Edit > Ctrl+{letter} needs ydotool (with ydotoold running), wtype or xdotool installed"
        );
    });
}

/// The "Edit" title next to "File", with Cut / Copy / Paste. Built exactly
/// like the File menu so it looks, opens and aligns the same way.
fn edit_menu() -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");
    for (label, letter, code, shift) in [
        ("Undo", "z", 44_u32, false),
        ("Redo", "z", 44, true),
    ] {
        append_menu_item(
            &menu,
            &popover,
            label,
            Some(Box::new(move || send_ctrl_shortcut(letter, code, shift))),
        );
    }
    append_separator(&menu);
    for (label, letter, code) in [("Cut", "x", 45_u32), ("Copy", "c", 46), ("Paste", "v", 47)] {
        append_menu_item(
            &menu,
            &popover,
            label,
            Some(Box::new(move || send_ctrl_shortcut(letter, code, false))),
        );
    }
    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&popover);

    let title = gtk::Label::new(Some("Edit"));
    title.add_css_class("lunar-menubar-item");
    let button = gtk::MenuButton::new();
    button.set_size_request(45, HEIGHT);
    button.set_child(Some(&title));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-active-app-button");
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);

    let button_weak = button.downgrade();
    let title_weak = title.downgrade();
    popover.connect_show(move |popover| {
        if let (Some(button), Some(title)) = (button_weak.upgrade(), title_weak.upgrade()) {
            align_active_menu(&button, &title, popover);
        }
    });
    button
}

/// Opens `uri` in the default file manager, detached from the shell.
fn open_in_file_manager(uri: &str) {
    if let Some(app) = gtk::gio::AppInfo::default_for_type("inode/directory", false) {
        if let Some(desktop_file) = crate::dock::desktop_file_for(&app) {
            if crate::dock::spawn_detached(
                std::ffi::OsStr::new("gio"),
                &[
                    std::ffi::OsStr::new("launch"),
                    desktop_file.as_os_str(),
                    std::ffi::OsStr::new(uri),
                ],
            )
            .is_ok()
            {
                return;
            }
        }
        if app
            .launch_uris(&[uri], None::<&gtk::gio::AppLaunchContext>)
            .is_ok()
        {
            return;
        }
    }
    if let Err(error) =
        gtk::gio::AppInfo::launch_default_for_uri(uri, None::<&gtk::gio::AppLaunchContext>)
    {
        eprintln!("lunar-desktop: could not open {uri}: {error}");
    }
}

/// URI of a folder inside the home directory (XDG user directory if set).
fn home_folder_uri(directory: glib::UserDirectory, fallback_name: &str) -> String {
    let path = glib::user_special_dir(directory).unwrap_or_else(|| glib::home_dir().join(fallback_name));
    gtk::gio::File::for_path(path).uri().to_string()
}

/// The "Go" menu, shown next to "Edit" only while Finder is the active app.
/// Built like the File and Edit menus.
fn go_menu() -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");

    let places: Vec<(&str, Box<dyn Fn() -> String>)> = vec![
        ("Recent", Box::new(|| "recentlyused:/".to_string())),
        (
            "Documents",
            Box::new(|| home_folder_uri(glib::UserDirectory::Documents, "Documents")),
        ),
        (
            "Desktop",
            Box::new(|| home_folder_uri(glib::UserDirectory::Desktop, "Desktop")),
        ),
        (
            "Downloads",
            Box::new(|| home_folder_uri(glib::UserDirectory::Downloads, "Downloads")),
        ),
        (
            "Home",
            Box::new(|| gtk::gio::File::for_path(glib::home_dir()).uri().to_string()),
        ),
        ("Computer", Box::new(|| "computer:/".to_string())),
        ("Applications", Box::new(|| "applications:/".to_string())),
    ];
    for (index, (label, uri)) in places.into_iter().enumerate() {
        if index == 1 {
            append_separator(&menu);
        }
        append_menu_item(
            &menu,
            &popover,
            label,
            Some(Box::new(move || open_in_file_manager(&uri()))),
        );
    }
    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&popover);

    let title = gtk::Label::new(Some("Go"));
    title.add_css_class("lunar-menubar-item");
    let button = gtk::MenuButton::new();
    button.set_size_request(36, HEIGHT);
    button.set_child(Some(&title));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-active-app-button");
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);

    let button_weak = button.downgrade();
    let title_weak = title.downgrade();
    popover.connect_show(move |popover| {
        if let (Some(button), Some(title)) = (button_weak.upgrade(), title_weak.upgrade()) {
            align_active_menu(&button, &title, popover);
        }
    });
    button
}

type WindowTarget = std::rc::Rc<std::cell::RefCell<Option<String>>>;

/// The "Window" title with Minimise, Fill, Centre, a "Move & resize" submenu
/// and Bring All to Front. Built like the File, Edit and Go menus. `target`
/// holds the id of the focused application window (None for Finder).
fn window_menu(target: &WindowTarget) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");

    let simple = |label: &str, action: &'static str, menu: &gtk::Box, popover: &gtk::Popover| {
        let target = std::rc::Rc::clone(target);
        append_menu_item(
            menu,
            popover,
            label,
            Some(Box::new(move || {
                crate::dock::run_active_window_action(target.borrow().clone(), action)
            })),
        );
    };
    simple("Minimise", "minimise", &menu, &popover);
    simple("Fill", "fill", &menu, &popover);
    simple("Centre", "centre", &menu, &popover);

    // "Move & resize  >" opens a second popover to the right.
    let sub_button = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let text = gtk::Label::new(Some("Move & resize"));
    text.set_halign(gtk::Align::Start);
    text.set_hexpand(true);
    let arrow = gtk::Label::new(Some("\u{203A}"));
    row.append(&text);
    row.append(&arrow);
    sub_button.set_child(Some(&row));
    sub_button.set_halign(gtk::Align::Fill);
    sub_button.add_css_class("lunar-menu-item");
    menu.append(&sub_button);

    let sub = gtk::Popover::new();
    sub.set_has_arrow(false);
    sub.set_position(gtk::PositionType::Right);
    sub.set_offset(4, -4);
    sub.add_css_class("lunar-menu-popover");
    sub.set_autohide(false);
    sub.set_parent(&sub_button);
    {
        let sub = sub.clone();
        sub_button.connect_destroy(move |_| sub.unparent());
    }
    let sub_menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sub_menu.add_css_class("lunar-system-menu");
    sub_menu.add_css_class("active-app-menu");
    let sub_item = |label: &str, action: &'static str| {
        let target = std::rc::Rc::clone(target);
        let main = popover.downgrade();
        append_menu_item(
            &sub_menu,
            &sub,
            label,
            Some(Box::new(move || {
                if let Some(main) = main.upgrade() {
                    main.popdown();
                }
                crate::dock::run_active_window_action(target.borrow().clone(), action)
            })),
        );
    };
    // Section headings are disabled (dimmed) items.
    append_menu_item(&sub_menu, &sub, "Halves", None);
    sub_item("Left", "left");
    sub_item("Right", "right");
    sub_item("Top", "top");
    sub_item("Bottom", "bottom");
    append_separator(&sub_menu);
    append_menu_item(&sub_menu, &sub, "Quarters", None);
    sub_item("Top Left", "top-left");
    sub_item("Top Right", "top-right");
    sub_item("Bottom Left", "bottom-left");
    sub_item("Bottom Right", "bottom-right");
    sub.set_child(Some(&sub_menu));
    crate::liquid_glass::glass_popover(&sub, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&sub);

    {
        let sub = sub.clone();
        sub_button.connect_clicked(move |_| sub.popup());
    }
    let open_on_hover = gtk::EventControllerMotion::new();
    {
        let sub = sub.clone();
        open_on_hover.connect_enter(move |_, _, _| sub.popup());
    }
    sub_button.add_controller(open_on_hover);
    {
        let sub = sub.clone();
        popover.connect_closed(move |_| sub.popdown());
    }

    append_separator(&menu);
    simple("Bring All to Front", "front", &menu, &popover);

    // Hovering any other item closes the submenu again.
    let mut child = menu.first_child();
    while let Some(widget) = child {
        if widget.type_() == gtk::Button::static_type() && &widget != sub_button.upcast_ref::<gtk::Widget>() {
            let leave = gtk::EventControllerMotion::new();
            let sub = sub.clone();
            leave.connect_enter(move |_, _, _| sub.popdown());
            widget.add_controller(leave);
        }
        child = widget.next_sibling();
    }

    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&popover);

    let title = gtk::Label::new(Some("Window"));
    title.add_css_class("lunar-menubar-item");
    let button = gtk::MenuButton::new();
    button.set_size_request(64, HEIGHT);
    button.set_child(Some(&title));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-active-app-button");
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);

    let button_weak = button.downgrade();
    let title_weak = title.downgrade();
    popover.connect_show(move |popover| {
        if let (Some(button), Some(title)) = (button_weak.upgrade(), title_weak.upgrade()) {
            align_active_menu(&button, &title, popover);
        }
    });
    button
}

/// Placeholder: replace with the real documentation address.
const DOCUMENTATION_URL: &str = "https://example.com/lunar-desktop/docs";

/// A top-level menu that Help can search: how its title is shown in results
/// (the app-name menu changes with the focused app) and its bar button.
type SearchableMenu = (Rc<dyn Fn() -> String>, gtk::MenuButton);

struct SearchHit {
    menu: String,
    item: String,
    /// Opens the menu (and submenu) the item lives in.
    open: Rc<dyn Fn()>,
}

fn button_label(button: &gtk::Button) -> Option<String> {
    let child = button.child()?;
    if let Some(label) = child.downcast_ref::<gtk::Label>() {
        return Some(label.text().to_string());
    }
    child
        .first_child()?
        .downcast::<gtk::Label>()
        .ok()
        .map(|label| label.text().to_string())
}

/// Walks a menu's widgets collecting its enabled items, including the items
/// of submenus parented to an item.
fn collect_hits(content: &gtk::Widget, title: &str, open: &Rc<dyn Fn()>, hits: &mut Vec<SearchHit>) {
    let mut child = content.first_child();
    while let Some(widget) = child {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.is_sensitive() {
                if let Some(text) = button_label(button) {
                    hits.push(SearchHit {
                        menu: title.to_string(),
                        item: text,
                        open: Rc::clone(open),
                    });
                }
            }
            let mut inner = button.first_child();
            while let Some(candidate) = inner {
                if let Some(sub) = candidate.downcast_ref::<gtk::Popover>() {
                    if let Some(sub_content) = sub.child() {
                        let parent_open = Rc::clone(open);
                        let sub = sub.clone();
                        let open_sub: Rc<dyn Fn()> = Rc::new(move || {
                            parent_open();
                            let sub = sub.clone();
                            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                                sub.popup()
                            });
                        });
                        collect_hits(&sub_content, title, &open_sub, hits);
                    }
                }
                inner = candidate.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
}

/// The "Help" title: a search field that finds menu bar items (picking a
/// result opens the menu it belongs to) and a placeholder documentation link.
/// Built like the other menus.
///
/// The menu bar normally never takes keyboard focus; while Help is open it
/// asks for it on demand so the search field can be typed in, and `frozen`
/// tells the focused-app tracker to hold still meanwhile.
fn help_menu(
    menus: Vec<SearchableMenu>,
    window_slot: Rc<std::cell::RefCell<Option<glib::WeakRef<gtk::Window>>>>,
    frozen: Rc<std::cell::Cell<bool>>,
) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search"));
    search.add_css_class("lunar-help-search");
    menu.append(&search);
    let results = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.append(&results);
    append_separator(&menu);
    append_menu_item(
        &menu,
        &popover,
        "Lunar Desktop Documentation",
        Some(Box::new(|| {
            if let Err(error) = gtk::gio::AppInfo::launch_default_for_uri(
                DOCUMENTATION_URL,
                None::<&gtk::gio::AppLaunchContext>,
            ) {
                eprintln!("lunar-desktop: could not open {DOCUMENTATION_URL}: {error}");
            }
        })),
    );
    popover.set_child(Some(&menu));
    crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());
    context_menu::attach_blur(&popover);

    {
        let results = results.downgrade();
        let popover = popover.downgrade();
        search.connect_search_changed(move |entry| {
            let (Some(results), Some(popover)) = (results.upgrade(), popover.upgrade()) else {
                return;
            };
            while let Some(child) = results.first_child() {
                results.remove(&child);
            }
            let query = entry.text().trim().to_lowercase();
            if query.is_empty() {
                return;
            }
            let mut hits = Vec::new();
            for (title, button) in &menus {
                if !button.is_visible() {
                    continue;
                }
                let Some(content) = button.popover().and_then(|popover| popover.child()) else {
                    continue;
                };
                let weak = button.downgrade();
                let open: Rc<dyn Fn()> = Rc::new(move || {
                    if let Some(button) = weak.upgrade() {
                        button.popup();
                    }
                });
                collect_hits(&content, &title(), &open, &mut hits);
            }
            hits.retain(|hit| hit.item.to_lowercase().contains(&query));
            hits.truncate(8);
            if hits.is_empty() {
                append_menu_item(&results, &popover, "No Results", None);
            }
            for hit in hits {
                let open = Rc::clone(&hit.open);
                append_menu_item(
                    &results,
                    &popover,
                    &format!("{} \u{203A} {}", hit.menu, hit.item),
                    Some(Box::new(move || {
                        let open = Rc::clone(&open);
                        glib::timeout_add_local_once(Duration::from_millis(120), move || open());
                    })),
                );
            }
        });
    }

    {
        let window_slot = Rc::clone(&window_slot);
        let frozen = Rc::clone(&frozen);
        let search = search.downgrade();
        popover.connect_show(move |_| {
            frozen.set(true);
            if let Some(window) = window_slot.borrow().as_ref().and_then(|window| window.upgrade()) {
                window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
            if let Some(search) = search.upgrade() {
                search.set_text("");
                search.grab_focus();
            }
        });
    }
    popover.connect_closed(move |_| {
        frozen.set(false);
        if let Some(window) = window_slot.borrow().as_ref().and_then(|window| window.upgrade()) {
            window.set_keyboard_mode(KeyboardMode::None);
        }
    });

    let title = gtk::Label::new(Some("Help"));
    title.add_css_class("lunar-menubar-item");
    let button = gtk::MenuButton::new();
    button.set_size_request(45, HEIGHT);
    button.set_child(Some(&title));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-active-app-button");
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);

    let button_weak = button.downgrade();
    let title_weak = title.downgrade();
    popover.connect_show(move |popover| {
        if let (Some(button), Some(title)) = (button_weak.upgrade(), title_weak.upgrade()) {
            align_active_menu(&button, &title, popover);
        }
    });
    button
}

fn fixed_title(title: &str) -> Rc<dyn Fn() -> String> {
    let title = title.to_string();
    Rc::new(move || title.clone())
}

fn system_menu(popover: &gtk::Popover, app: &gtk::Application) -> gtk::Box {
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");

    let about_app = app.clone();
    append_menu_item(
        &menu,
        popover,
        "About This System",
        Some(Box::new(move || crate::about::present(&about_app))),
    );
    append_separator(&menu);
    let settings_app = app.clone();
    append_menu_item(
        &menu,
        popover,
        "System Settings",
        Some(Box::new(move || crate::settings::present(&settings_app))),
    );
    append_separator(&menu);
    append_menu_item(
        &menu,
        popover,
        "Force Quit",
        Some(Box::new(|| {
            spawn_first_available(&["plasma-systemmonitor", "ksysguard"]);
        })),
    );
    append_separator(&menu);
    append_menu_item(
        &menu,
        popover,
        "Sleep",
        Some(Box::new(|| run_system_action("systemctl", "suspend"))),
    );
    append_menu_item(
        &menu,
        popover,
        "Restart",
        Some(Box::new(|| run_system_action("systemctl", "reboot"))),
    );
    append_menu_item(
        &menu,
        popover,
        "Shut Down",
        Some(Box::new(|| run_system_action("systemctl", "poweroff"))),
    );
    append_separator(&menu);

    let username = std::env::var("USER").unwrap_or_else(|_| "User".to_string());
    append_menu_item(
        &menu,
        popover,
        &format!("Lock ({username})"),
        Some(Box::new(|| run_system_action("loginctl", "lock-session"))),
    );
    append_menu_item(
        &menu,
        popover,
        &format!("Log Out ({username})"),
        Some(Box::new(run_logout)),
    );

    menu
}

/// Distance from the active-app menu's left edge to the text of its items:
/// `.active-app-menu` padding (6px) + `.active-app-menu button.lunar-menu-item`
/// padding (10px) in style.rs. Keep in sync with those rules.
const ACTIVE_MENU_TEXT_INSET: f32 = 16.0;

/// Aligns the text of the active app's menu items with the app name in the bar
/// (the menu box itself then sticks out to the left by the inset, as on macOS).
///
/// GTK centres a bottom popover on its button, and both the button (it hugs the
/// app name) and the menu ("Quit <name>" can widen it) change size with the name
/// of the focused app. So this must run every time the menu is about to open,
/// not once at start-up, and it measures the widgets instead of assuming sizes.
fn align_active_menu(button: &gtk::MenuButton, label: &gtk::Label, popover: &gtk::Popover) {
    let button_width = button.width();
    let Some(menu) = popover.child() else {
        return;
    };
    if button_width <= 0 {
        return;
    }

    // The label's left edge relative to the button.
    let button_widget: &gtk::Widget = button.upcast_ref();
    let Some(label_left) = label.compute_bounds(button_widget).map(|b| b.x()) else {
        return;
    };

    // The menu's natural width, including its CSS padding and minimum width.
    let (_, menu_width, _, _) = menu.measure(gtk::Orientation::Horizontal, -1);

    // Centred: menu_left = (button_width - menu_width) / 2 + offset, measured
    // from the button's left edge. We want the item text, which starts
    // ACTIVE_MENU_TEXT_INSET inside the menu, to start at label_left.
    let offset_x =
        label_left - ACTIVE_MENU_TEXT_INSET + (menu_width - button_width) as f32 / 2.0;

    popover.set_offset(offset_x.round() as i32, 2);
}

fn find_by_class(widget: &gtk::Widget, class: &str) -> Option<gtk::Widget> {
    if widget.has_css_class(class) {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_by_class(&current, class) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

/// How far (logical px, per pass) the wallpaper under the bar is blurred. Big
/// enough that the bar reads as soft blocks of the wallpaper's colours.
const BACKDROP_BLUR: usize = 20;

/// Gives the bar its solid colours: the wallpaper behind it, blurred. Skipped
/// when the backdrop is already up to date for this output (same size and
/// scale) unless `force` is set, which a wallpaper change needs.
pub fn set_backdrop(
    window: &gtk::Window,
    image: &crate::wallpaper::Image,
    monitor: &gdk::Monitor,
    force: bool,
) {
    let Some(strip) = find_by_class(window.upcast_ref(), "menubar-glass").and_downcast::<GlassStrip>() else {
        return;
    };
    let geometry = monitor.geometry();
    let scale = monitor.scale_factor().max(1);
    let key = (geometry.width(), geometry.height(), scale);
    if !force && strip.backdrop_key() == Some(key) {
        return;
    }
    if geometry.width() <= 0 || geometry.height() <= 0 {
        return;
    }
    let (width, scale_px) = ((geometry.width() * scale) as usize, scale as usize);
    let radius = BACKDROP_BLUR * scale_px;
    // The bar plus the rows below it that the blur reads from.
    let rows = HEIGHT as usize * scale_px + crate::frost::PASSES * radius;
    let pixels = crate::frost::render(
        &image.texture,
        (geometry.width() as f32, geometry.height() as f32),
        width,
        rows,
        scale_px,
        radius,
    );
    let texture = gdk::MemoryTexture::new(
        width as i32,
        rows as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(pixels),
        width * 4,
    );
    strip.set_backdrop(texture.upcast(), scale, key);
}

/// Switches the bar between white text and near-black text (and the matching
/// glass wash and logo) to suit the wallpaper behind it, like macOS does. This
/// is independent of the Dark Mode setting, which only affects the menus.
pub fn set_tone(window: &gtk::Window, tone: crate::wallpaper::BarTone) {
    let (add, remove) = match tone {
        crate::wallpaper::BarTone::LightText => ("bar-light-text", "bar-dark-text"),
        crate::wallpaper::BarTone::DarkText => ("bar-dark-text", "bar-light-text"),
    };
    if window.has_css_class(add) {
        return;
    }
    window.remove_css_class(remove);
    window.add_css_class(add);
    // The glass paints its tint in snapshot(); ask it for a new frame.
    if let Some(glass) = find_by_class(window.upcast_ref(), "menubar-glass") {
        glass.queue_draw();
    }
}

/// Creates (but does not present) the menu bar window for `monitor`.
pub fn window(
    app: &gtk::Application,
    monitor: &gdk::Monitor,
    tracker: Option<KWinWindows>,
) -> gtk::Window {
    let glass = GlassStrip::new();
    glass.add_css_class("menubar-glass");
    glass.set_height_request(HEIGHT);
    // The full-width visual surface must not win pointer picking over the
    // interactive MenuButton layered above it.
    glass.set_can_target(false);

    let content = gtk::Overlay::new();
    content.set_child(Some(&glass));
    // Set once the window exists; lets Help ask for keyboard focus.
    let window_slot: Rc<std::cell::RefCell<Option<glib::WeakRef<gtk::Window>>>> =
        Rc::new(std::cell::RefCell::new(None));
    // True while Help is open: the focused-app tracker pauses.
    let help_open = Rc::new(std::cell::Cell::new(false));
    {
        let logo = gtk::Overlay::new();
        logo.add_css_class("lunar-kde-logo");
        logo.set_size_request(20, 20);
        logo.set_can_target(false);
        let light_logo = gtk::Image::new();
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
            "../assets/apple-logo-light.png"
        ))) {
            light_logo.set_paintable(Some(&texture));
        }
        light_logo.set_pixel_size(20);
        light_logo.set_can_target(false);
        light_logo.add_css_class("lunar-kde-logo-light");
        logo.set_child(Some(&light_logo));
        let dark_logo = gtk::Image::new();
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
            "../assets/apple-logo-dark.png"
        ))) {
            dark_logo.set_paintable(Some(&texture));
        }
        dark_logo.set_pixel_size(20);
        dark_logo.set_can_target(false);
        dark_logo.add_css_class("lunar-kde-logo-dark");
        logo.add_overlay(&dark_logo);
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_offset(6, 2);
        popover.add_css_class("lunar-menu-popover");
        popover.set_child(Some(&system_menu(&popover, app)));
        crate::liquid_glass::glass_popover(&popover, crate::liquid_glass::Material::menu());

        context_menu::attach_blur(&popover);

        let menu_button = gtk::MenuButton::new();
        menu_button.set_child(Some(&logo));
        menu_button.set_popover(Some(&popover));
        menu_button.set_size_request(34, HEIGHT);
        menu_button.set_can_target(true);
        menu_button.set_halign(gtk::Align::Start);
        menu_button.set_valign(gtk::Align::Center);
        menu_button.set_margin_start(6);
        menu_button.set_tooltip_text(Some("KDE"));
        menu_button.add_css_class("lunar-menu-button");
        let menu_button_weak = menu_button.downgrade();
        popover.connect_visible_notify(move |popover| {
            if let Some(menu_button) = menu_button_weak.upgrade() {
                if popover.is_visible() {
                    menu_button.add_css_class("menu-open");
                } else {
                    menu_button.remove_css_class("menu-open");
                }
            }
        });
        let active_app = gtk::Label::new(Some("Finder"));
        active_app.add_css_class("lunar-active-app");
        let active_menu = gtk::Popover::new();
        active_menu.set_has_arrow(false);
        active_menu.set_position(gtk::PositionType::Bottom);
        active_menu.set_offset(0, 2);
        active_menu.add_css_class("lunar-menu-popover");
        let active_menu_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        active_menu_content.add_css_class("lunar-system-menu");
        active_menu_content.add_css_class("active-app-menu");
        let quit_action = gtk::Button::new();
        let quit_action_label = gtk::Label::new(Some("Quit Finder"));
        quit_action_label.set_halign(gtk::Align::Start);
        quit_action.set_child(Some(&quit_action_label));
        quit_action.set_halign(gtk::Align::Fill);
        quit_action.add_css_class("lunar-menu-item");
        quit_action.add_css_class("lunar-context-menu-item");
        quit_action.set_sensitive(false);
        active_menu_content.append(&quit_action);
        active_menu.set_child(Some(&active_menu_content));
        crate::liquid_glass::glass_popover(&active_menu, crate::liquid_glass::Material::menu());
        context_menu::attach_blur(&active_menu);

        let quit_target = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
        let quit_target_for_action = std::rc::Rc::clone(&quit_target);
        let active_menu_for_action = active_menu.downgrade();
        quit_action.connect_clicked(move |_| {
            eprintln!("lunar-desktop: app menu Quit item clicked");
            if let Some(target) = quit_target_for_action.borrow().clone() {
                crate::dock::run_window_action(target, "quit");
            }
            if let Some(popover) = active_menu_for_action.upgrade() {
                popover.popdown();
            }
        });

        let active_app_button = gtk::MenuButton::new();

        let active_app_weak = active_app.downgrade();
        let active_button_weak = active_app_button.downgrade();
        let active_menu_weak = active_menu.downgrade();

        // Re-align on every open: the app name (and so the button and menu
        // widths) changes whenever the focused application does.
        active_menu.connect_show(move |_| {
            if let (Some(label), Some(button), Some(popover)) = (
                active_app_weak.upgrade(),
                active_button_weak.upgrade(),
                active_menu_weak.upgrade(),
            ) {
                align_active_menu(&button, &label, &popover);
            }
        });

        // Diagnostics, as for the File menu: does the app-name menu map and close?
        active_menu.connect_map(|popover| {
            eprintln!(
                "lunar-desktop: app menu opened (child size {}x{})",
                popover.child().map_or(0, |child| child.width()),
                popover.child().map_or(0, |child| child.height())
            );
        });
        active_menu.connect_unmap(|popover| {
            eprintln!(
                "lunar-desktop: app menu unmapped (visible={} root_focus={:?})",
                popover.is_visible(),
                popover.root().and_then(|root| root.focus()).map(|w| w.type_().name())
            );
        });
        active_menu.connect_closed(|_| eprintln!("lunar-desktop: app menu closed signal"));

        // Configured exactly like the File button, whose menu does open.
        active_menu.set_autohide(true);
        active_app_button.set_can_target(true);
        active_app_button.set_child(Some(&active_app));
        active_app_button.set_popover(Some(&active_menu));
        active_app_button.set_halign(gtk::Align::Start);
        active_app_button.set_valign(gtk::Align::Center);
        active_app_button.add_css_class("lunar-active-app-button");
        // Same pill highlight while its menu is open as the File button.
        active_app_button.add_css_class("lunar-menubar-item-button");

        let menu_group = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        menu_group.set_halign(gtk::Align::Start);
        menu_group.set_valign(gtk::Align::Center);
        menu_group.set_hexpand(false);
        menu_group.set_margin_start(6);
        menu_group.append(&menu_button);
        menu_group.append(&active_app_button);
        let new_window_target = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
        let (file_button, new_window_label) = file_menu(&new_window_target);
        menu_group.append(&file_button);
        let edit_button = edit_menu();
        menu_group.append(&edit_button);
        // Finder is the app shown at start-up, so Go starts visible.
        let go_button = go_menu();
        menu_group.append(&go_button);
        let window_target: WindowTarget = std::rc::Rc::new(std::cell::RefCell::new(None));
        let window_button = window_menu(&window_target);
        menu_group.append(&window_button);
        let active_app_for_title = active_app.downgrade();
        let searchable: Vec<SearchableMenu> = vec![
            (fixed_title("System"), menu_button.clone()),
            (
                Rc::new(move || {
                    active_app_for_title
                        .upgrade()
                        .map(|label| label.text().to_string())
                        .unwrap_or_default()
                }),
                active_app_button.clone(),
            ),
            (fixed_title("File"), file_button.clone()),
            (fixed_title("Edit"), edit_button.clone()),
            (fixed_title("Go"), go_button.clone()),
            (fixed_title("Window"), window_button.clone()),
        ];
        menu_group.append(&help_menu(
            searchable,
            Rc::clone(&window_slot),
            Rc::clone(&help_open),
        ));
        content.add_overlay(&menu_group);

        if let Some(tracker) = tracker {
            let active_app_weak = active_app.downgrade();
            let quit_label_weak = quit_action_label.downgrade();
            let quit_button_weak = quit_action.downgrade();
            let new_window_label_weak = new_window_label.downgrade();
            let go_button_weak = go_button.downgrade();
            let window_target = std::rc::Rc::clone(&window_target);
            let help_open = Rc::clone(&help_open);
            let quit_target = std::rc::Rc::clone(&quit_target);
            let last_revision = std::cell::Cell::new(u64::MAX);
            glib::timeout_add_local(Duration::from_millis(200), move || {
                // Holding keyboard focus for the Help search field makes the
                // compositor report no focused app; wait until it closes.
                if help_open.get() {
                    return glib::ControlFlow::Continue;
                }
                let revision = tracker.revision();
                if last_revision.replace(revision) != revision {
                    let snapshot = tracker.snapshot();
                    // A menu of our own just took focus (e.g. this very menu
                    // opening): keep showing the app that was frontmost,
                    // instead of rewriting the label under the open menu.
                    if snapshot.windows.iter().any(|w| w.focused && is_own_popup(w)) {
                        eprintln!("lunar-desktop: focus is on a shell popup; app name left as is");
                        return glib::ControlFlow::Continue;
                    }
                    let (name, target) = active_application(&snapshot);
                    let identity = new_window_identity(&snapshot);
                    if let Some(label) = new_window_label_weak.upgrade() {
                        label.set_text(new_window_title(identity.as_deref()));
                    }
                    new_window_target.replace(identity);
                    window_target.replace(
                        snapshot
                            .windows
                            .iter()
                            .find(|window| {
                                window.focused && !is_own_window(window)
                            })
                            .map(|window| window.window_id.clone()),
                    );
                    // Go only belongs to Finder.
                    if let Some(go_button) = go_button_weak.upgrade() {
                        let finder = name == "Finder";
                        if !finder {
                            go_button.popdown();
                        }
                        go_button.set_visible(finder);
                    }
                    eprintln!("lunar-desktop: focused app is now {name:?} (revision {revision})");
                    if let Some(label) = active_app_weak.upgrade() {
                        label.set_text(&name);
                    } else {
                        return glib::ControlFlow::Break;
                    }
                    if let Some(label) = quit_label_weak.upgrade() {
                        label.set_text(&format!("Quit {name}"));
                    }
                    if let Some(button) = quit_button_weak.upgrade() {
                        button.set_sensitive(target.is_some());
                    }
                    quit_target.replace(target);
                }
                glib::ControlFlow::Continue
            });
        }
    }

    // Right-hand status area: Wi-Fi, then the clock. It must stay targetable:
    // in GTK4 a non-targetable container makes its children unclickable too.
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    status.set_halign(gtk::Align::End);
    status.set_valign(gtk::Align::Center);
    status.append(&wifi_menu());
    status.append(&crate::control_center::control_center_button(app));
    status.append(&clock_label());
    content.add_overlay(&status);

    let window = gtk::Window::builder()
        .application(app)
        .title("Lunar Menu Bar")
        .decorated(false)
        .default_height(HEIGHT)
        .css_classes(["lunar-menubar"])
        .child(&content)
        .build();
    crate::preferences::apply_dark_mode(&window);

    let geometry = monitor.geometry();
    window.set_default_size(geometry.width(), HEIGHT);
    let gdk_wayland = gdk::Display::default().is_some_and(|display| display.backend().is_wayland());
    let layer_protocol_version = gtk4_layer_shell::protocol_version();
    let library_support_check = gtk4_layer_shell::is_supported();
    let layer_shell_available = gdk_wayland && library_support_check;
    if layer_shell_available {
        // Verify GTK initialized a real layer surface before depending on its
        // anchors and exclusive zone. A normal GtkWindow is not a panel.
        window.init_layer_shell();
    }
    let layer_shell_active = layer_shell_available && window.is_layer_window();
    eprintln!(
        "lunar-desktop: menubar layer-shell probe gdk_wayland={gdk_wayland} protocol_version={layer_protocol_version} available={layer_shell_available} initialized={layer_shell_active} library_support_check={library_support_check}"
    );
    if layer_shell_active {
        eprintln!(
            "lunar-desktop: menubar uses Wayland overlay layer with {HEIGHT}px exclusive zone"
        );
        // An exclusive zone makes KWin's maximized work area begin below the
        // menubar. Plasma's old panel-behavior hint is deprecated in Plasma 6.
        window.set_namespace(Some("lunar-menubar"));
        window.set_monitor(Some(monitor));
        // Fullscreen clients may be stacked above the top layer; overlay is
        // the layer-shell level intended to remain visible over them.
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_exclusive_zone(HEIGHT);
        window.set_keyboard_mode(KeyboardMode::None);
        *window_slot.borrow_mut() = Some(window.downgrade());
    } else {
        eprintln!("lunar-desktop: menubar layer-shell unavailable; using Plasma panel fallback (fullscreen windows may cover it)");
        // Retain the existing Plasma-shell role as a compositor fallback.
        kde_shell::configure_window(&window, monitor, Role::Panel, geometry.x(), geometry.y());
    }
    enable_hover_switching(&window);
    // The bar deliberately asks KWin for no backdrop effect, so the glass
    // plugin (blur + refraction) leaves it alone; it keeps only its own tint
    // and hairline (`GlassStrip`). Its menus still blur, see `attach_blur`.
    window
}

fn collect_menu_buttons(widget: &gtk::Widget, found: &mut Vec<gtk::MenuButton>) {
    if let Some(button) = widget.downcast_ref::<gtk::MenuButton>() {
        found.push(button.clone());
        return;
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_menu_buttons(&current, found);
        child = current.next_sibling();
    }
}

/// Like macOS: while one menu bar menu is open, moving the pointer onto another
/// title opens that one straight away. An open popover grabs the pointer, so
/// the titles get no hover events; instead the pointer position is polled.
fn enable_hover_switching(window: &gtk::Window) {
    let mut buttons = Vec::new();
    if let Some(content) = window.child() {
        collect_menu_buttons(&content, &mut buttons);
    }
    let window = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let Some(window) = window.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let Some(open) = buttons
            .iter()
            .find(|button| button.popover().is_some_and(|popover| popover.is_visible()))
        else {
            return glib::ControlFlow::Continue;
        };
        let Some(pointer) = gdk::Display::default()
            .and_then(|display| display.default_seat())
            .and_then(|seat| seat.pointer())
        else {
            return glib::ControlFlow::Continue;
        };
        let (surface, x, y) = pointer.surface_at_position();
        if surface.is_none() || surface != window.surface() {
            return glib::ControlFlow::Continue;
        }
        let target = window
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .and_then(|widget| widget.ancestor(gtk::MenuButton::static_type()))
            .and_then(|widget| widget.downcast::<gtk::MenuButton>().ok());
        if let Some(target) = target {
            if &target != open && buttons.contains(&target) && target.is_sensitive() {
                eprintln!(
                    "lunar-desktop: hover switch closes {:?} for {:?}",
                    open.css_classes(),
                    target.css_classes()
                );
                open.popdown();
                target.popup();
            }
        }
        glib::ControlFlow::Continue
    });
}
