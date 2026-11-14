//! The Wi-Fi page of System Settings: the radio switch, the connected network,
//! the known networks and the other networks in range. Everything comes from
//! NetworkManager through `nmcli`, like the menu bar's Wi-Fi menu.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use gtk::{gdk, glib};
use gtk::prelude::*;

use crate::context_menu::ContextMenu;
use crate::menubar::{open_network_settings, split_terse};
use crate::settings::{section_heading, settings_group};

const LEARN_MORE_URL: &str = "https://networkmanager.dev/";

// ------------------------------------------------------------------- data

#[derive(Clone)]
struct Network {
    ssid: String,
    /// NetworkManager's name for the saved profile, if the network is known.
    profile: Option<String>,
    signal: u8,
    active: bool,
    secured: bool,
}

struct State {
    available: bool,
    enabled: bool,
    networks: Vec<Network>,
}

fn nmcli_output(args: &[&str]) -> Option<String> {
    let output = Command::new("nmcli")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// SSIDs of saved profiles, by profile name (each is asked for once).
static PROFILE_SSIDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The SSID a saved Wi-Fi profile joins. Usually the same as its name, but a
/// profile can be called anything.
fn profile_ssid(profile: &str) -> String {
    let mut cache = PROFILE_SSIDS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(ssid) = cache.get(profile) {
        return ssid.clone();
    }
    let ssid = nmcli_output(&["-g", "802-11-wireless.ssid", "connection", "show", "id", profile])
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| profile.to_string());
    cache.insert(profile.to_string(), ssid.clone());
    ssid
}

/// Reads the radio state and the cached scan results. Blocking: run it off
/// the main thread.
fn read_state() -> State {
    let Some(radio) = nmcli_output(&["-t", "-f", "WIFI", "radio"]) else {
        return State { available: false, enabled: false, networks: Vec::new() };
    };
    let enabled = radio.trim() == "enabled";
    let mut networks: Vec<Network> = Vec::new();
    if enabled {
        // (ssid, profile name) of every saved Wi-Fi profile.
        let known: Vec<(String, String)> = nmcli_output(&["-t", "-f", "NAME,TYPE", "connection", "show"])
            .map(|text| {
                text.lines()
                    .filter_map(|line| {
                        let fields = split_terse(line);
                        let is_wifi = fields.get(1).is_some_and(|kind| kind == "802-11-wireless" || kind == "wifi");
                        is_wifi.then(|| (profile_ssid(&fields[0]), fields[0].clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if let Some(list) = nmcli_output(&[
            "-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "no",
        ]) {
            for line in list.lines() {
                let fields = split_terse(line);
                if fields.len() < 3 || fields[1].trim().is_empty() {
                    continue;
                }
                let network = Network {
                    profile: known
                        .iter()
                        .find(|(ssid, _)| *ssid == fields[1])
                        .map(|(_, profile)| profile.clone()),
                    ssid: fields[1].clone(),
                    signal: fields[2].trim().parse().unwrap_or(0),
                    active: fields[0].trim() == "*",
                    secured: fields.get(3).is_some_and(|security| !matches!(security.trim(), "" | "--")),
                };
                // One entry per SSID (an SSID is often broadcast by several access points).
                match networks.iter_mut().find(|other| other.ssid == network.ssid) {
                    Some(other) => {
                        other.active |= network.active;
                        other.secured |= network.secured;
                        other.signal = other.signal.max(network.signal);
                    }
                    None => networks.push(network),
                }
            }
        }
        networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
        networks.truncate(60);
    }
    State { available: true, enabled, networks }
}

/// Runs `nmcli` off the main thread and calls `done` (on the main thread)
/// with whether it succeeded.
fn run_nmcli(args: Vec<String>, done: impl FnOnce(bool) + 'static) {
    glib::spawn_future_local(async move {
        let ok = gtk::gio::spawn_blocking(move || {
            Command::new("nmcli")
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .await
        .unwrap_or(false);
        done(ok);
    });
}

pub(crate) fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

// ------------------------------------------------------------ page state

pub(crate) type Slot = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

/// Runs the page's refresh, if the page still exists.
pub(crate) fn refresh_now(slot: &Slot) {
    let refresh = slot.borrow().clone();
    if let Some(refresh) = refresh {
        refresh();
    }
}

pub(crate) fn refresh_later(slot: &Slot, millis: u64) {
    let slot = Rc::clone(slot);
    glib::timeout_add_local_once(Duration::from_millis(millis), move || refresh_now(&slot));
}

/// Asks for a fresh scan, then refreshes once its results are in.
fn rescan(slot: &Slot) {
    run_nmcli(strings(&["device", "wifi", "rescan"]), |_| {});
    refresh_later(slot, 3500);
}

#[derive(Clone)]
struct Ui {
    switch: gtk::Switch,
    /// Separator and row for the connected network, inside the top card.
    connected: gtk::Box,
    /// Known networks, other networks and the "Other…" button.
    lists: gtk::Box,
    /// True while the switch is set from NetworkManager's state, so that is
    /// not mistaken for the user flipping it.
    updating: Rc<Cell<bool>>,
    slot: Slot,
}

// ------------------------------------------------------------------ glyphs

/// A small drawing in the widget's text colour. `paint` gets the context,
/// the size, and `ink(alpha)`, which sets the source colour at a fraction of
/// the text colour's opacity.
pub(crate) fn glyph(
    width: i32,
    height: i32,
    paint: impl Fn(&gtk::cairo::Context, f64, f64, &dyn Fn(f64)) + 'static,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(width);
    area.set_content_height(height);
    area.set_valign(gtk::Align::Center);
    area.set_halign(gtk::Align::Center);
    area.add_css_class("wifi-glyph");
    area.set_draw_func(move |area, cr, w, h| {
        let color = area.color();
        let ink = |scale: f64| {
            cr.set_source_rgba(
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
                color.alpha() as f64 * scale,
            )
        };
        paint(cr, w as f64, h as f64, &ink);
    });
    area
}

/// The Wi-Fi symbol: a dot and three arcs, lit according to `signal` (0-100).
fn wifi_glyph(signal: u8) -> gtk::DrawingArea {
    let lit = match signal {
        0..=25 => 1,
        26..=50 => 2,
        _ => 3,
    };
    glyph(18, 15, move |cr, w, h, ink| {
        let (cx, cy) = (w / 2.0, h - 2.0);
        ink(1.0);
        cr.arc(cx, cy, 1.7, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.set_line_width(2.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        for (index, radius) in [4.8, 8.2, 11.6].into_iter().enumerate() {
            ink(if index < lit { 1.0 } else { 0.3 });
            cr.new_sub_path();
            cr.arc(cx, cy, radius, -0.75 * PI, -0.25 * PI);
            let _ = cr.stroke();
        }
    })
}

/// The padlock beside secured networks. Open networks get an empty slot of
/// the same size so the columns line up.
fn lock_glyph(secured: bool) -> gtk::DrawingArea {
    glyph(12, 14, move |cr, w, h, ink| {
        if !secured {
            return;
        }
        ink(1.0);
        let (x0, y0, x1, y1, r) = (0.8, 6.0, w - 0.8, h - 0.8, 2.0);
        cr.new_sub_path();
        cr.arc(x1 - r, y0 + r, r, -0.5 * PI, 0.0);
        cr.arc(x1 - r, y1 - r, r, 0.0, 0.5 * PI);
        cr.arc(x0 + r, y1 - r, r, 0.5 * PI, PI);
        cr.arc(x0 + r, y0 + r, r, PI, 1.5 * PI);
        cr.close_path();
        let _ = cr.fill();
        cr.set_line_width(1.7);
        cr.new_sub_path();
        cr.arc(w / 2.0, 6.2, 3.3, PI, 2.0 * PI);
        let _ = cr.stroke();
    })
}

/// The check mark in front of the connected network (an empty slot otherwise).
fn check_glyph(shown: bool) -> gtk::DrawingArea {
    glyph(16, 14, move |cr, _w, _h, ink| {
        if !shown {
            return;
        }
        ink(1.0);
        cr.set_line_width(2.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.move_to(2.0, 7.5);
        cr.line_to(6.0, 11.5);
        cr.line_to(14.0, 2.5);
        let _ = cr.stroke();
    })
}

/// A circle holding three dots: the per-network menu button.
pub(crate) fn more_glyph() -> gtk::DrawingArea {
    glyph(18, 18, |cr, w, h, ink| {
        let (cx, cy) = (w / 2.0, h / 2.0);
        ink(0.85);
        cr.set_line_width(1.3);
        cr.arc(cx, cy, 8.2, 0.0, 2.0 * PI);
        let _ = cr.stroke();
        for dx in [-3.6, 0.0, 3.6] {
            cr.arc(cx + dx, cy, 1.0, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    })
}

// --------------------------------------------------------------- widgets

pub(crate) fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

pub(crate) fn action_button(text: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("wifi-action");
    button.set_valign(gtk::Align::Center);
    button.connect_clicked(move |_| on_click());
    button
}

pub(crate) fn separator() -> gtk::Separator {
    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
    line.add_css_class("settings-row-separator");
    line
}

pub(crate) fn group(rows: &[gtk::Box]) -> gtk::Box {
    let group = settings_group(rows);
    group.add_css_class("wifi-group");
    group
}

pub(crate) fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Joins a known network from its saved profile, or an open/new one by name.
/// If that is not possible (a password is needed) the network settings open.
fn join(network: &Network, slot: &Slot) {
    let args = match &network.profile {
        Some(profile) => strings(&["connection", "up", "id", profile]),
        None => strings(&["device", "wifi", "connect", &network.ssid]),
    };
    let slot = Rc::clone(slot);
    run_nmcli(args, move |ok| {
        if !ok {
            open_network_settings();
        }
        refresh_later(&slot, 500);
    });
}

fn leave(network: &Network, slot: &Slot) {
    let name = network.profile.clone().unwrap_or_else(|| network.ssid.clone());
    let slot = Rc::clone(slot);
    run_nmcli(strings(&["connection", "down", "id", &name]), move |_| refresh_later(&slot, 500));
}

fn forget(network: &Network, slot: &Slot) {
    let Some(profile) = network.profile.clone() else {
        return;
    };
    let slot = Rc::clone(slot);
    run_nmcli(strings(&["connection", "delete", "id", &profile]), move |_| refresh_later(&slot, 500));
}

/// The circled-dots button and the menu it opens.
fn more_button(network: &Network, ui: &Ui) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("wifi-more");
    button.set_valign(gtk::Align::Center);
    button.set_child(Some(&more_glyph()));

    let menu = ContextMenu::new(gtk::PositionType::Bottom, &["settings-popup-menu"]);
    menu.set_parent(&button);
    if network.active {
        let (network, slot) = (network.clone(), Rc::clone(&ui.slot));
        menu.append_action("Disconnect", true, move || leave(&network, &slot));
    } else {
        let (network, slot) = (network.clone(), Rc::clone(&ui.slot));
        menu.append_action("Connect", true, move || join(&network, &slot));
    }
    if network.profile.is_some() {
        let (network, slot) = (network.clone(), Rc::clone(&ui.slot));
        menu.append_action("Forget This Network\u{2026}", true, move || forget(&network, &slot));
    }
    menu.append_separator();
    menu.append_action("Network Settings\u{2026}", true, open_network_settings);

    let popup = menu.clone();
    button.connect_clicked(move |button| popup.popup_at(button.width() / 2, button.height()));
    // The popover is a child of the button: let go of it before the button
    // is finalised (rows are rebuilt on every refresh).
    button.connect_destroy(move |_| menu.unparent());
    button
}

/// One row of the Known / Other Networks lists.
fn network_row(network: &Network, ui: &Ui) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("settings-row");
    row.add_css_class("wifi-row");

    let check = check_glyph(network.active);
    check.set_margin_end(10);
    let name = label(&network.ssid, &["settings-row-title"]);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let lock = lock_glyph(network.secured);
    lock.set_margin_end(8);
    let signal = wifi_glyph(network.signal);
    signal.set_margin_end(8);

    row.append(&check);
    row.append(&name);
    row.append(&lock);
    row.append(&signal);
    row.append(&more_button(network, ui));

    if !network.active {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let (network, slot) = (network.clone(), Rc::clone(&ui.slot));
        click.connect_released(move |_, _, _, _| join(&network, &slot));
        row.add_controller(click);
    }
    row
}

/// The connected network, under the Wi-Fi switch: name, green "Connected",
/// lock, signal and the Details button.
fn connected_row(network: &Network) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("settings-row");
    row.add_css_class("wifi-connected-row");

    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("wifi-dot");
    dot.set_valign(gtk::Align::Center);
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status.append(&dot);
    status.append(&label("Connected", &["wifi-status-text"]));

    let name = label(&network.ssid, &["settings-row-title"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&name);
    text.append(&status);

    let lock = lock_glyph(network.secured);
    lock.set_margin_end(8);
    let signal = wifi_glyph(network.signal);
    signal.set_margin_end(14);

    row.append(&text);
    row.append(&lock);
    row.append(&signal);
    row.append(&action_button("Details\u{2026}", open_network_settings));
    row
}

/// Rebuilds everything that depends on the scan.
fn apply(ui: &Ui, state: &State) {
    ui.updating.set(true);
    ui.switch.set_active(state.enabled);
    ui.switch.set_sensitive(state.available);
    ui.updating.set(false);

    clear(&ui.connected);
    clear(&ui.lists);
    ui.connected.set_visible(state.enabled);
    ui.lists.set_visible(state.enabled || !state.available);

    if !state.available {
        ui.lists.append(&label(
            "Wi-Fi is unavailable: NetworkManager could not be reached.",
            &["settings-row-subtitle"],
        ));
        return;
    }
    if !state.enabled {
        return;
    }

    if let Some(active) = state.networks.iter().find(|network| network.active) {
        ui.connected.append(&separator());
        ui.connected.append(&connected_row(active));
    }

    let known: Vec<&Network> = state.networks.iter().filter(|network| network.profile.is_some()).collect();
    let others: Vec<&Network> = state.networks.iter().filter(|network| network.profile.is_none()).collect();

    if !known.is_empty() {
        let heading = if known.len() == 1 { "Known Network" } else { "Known Networks" };
        ui.lists.append(&section_heading(heading));
        let rows: Vec<gtk::Box> = known.iter().map(|network| network_row(network, ui)).collect();
        ui.lists.append(&group(&rows));
    }
    if !others.is_empty() {
        ui.lists.append(&section_heading("Other Networks"));
        let rows: Vec<gtk::Box> = others.iter().map(|network| network_row(network, ui)).collect();
        ui.lists.append(&group(&rows));
    }

    let other = action_button("Other\u{2026}", open_network_settings);
    other.set_halign(gtk::Align::End);
    other.add_css_class("wifi-other-button");
    ui.lists.append(&other);
}

// ------------------------------------------------------------------- page

/// Fills `body` (the scrolling content of the Wi-Fi page).
pub fn build(body: &gtk::Box) {
    // ---- top card: switch, then the connected network ----
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("settings-group");
    card.add_css_class("wifi-group");

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.add_css_class("settings-row");
    top.add_css_class("wifi-header-row");

    let icon = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../assets/settings-icons/wifi.png"
    ))) {
        icon.set_paintable(Some(&texture));
    }
    icon.set_pixel_size(26);
    icon.set_valign(gtk::Align::Start);
    icon.add_css_class("wifi-header-icon");

    let title = label("Wi-Fi", &["settings-row-title"]);
    let subtitle = gtk::Label::new(None);
    subtitle.set_markup(&format!(
        "Set up Wi-Fi to wirelessly connect your Mac to the internet. Turn on Wi-Fi, then choose a network to join. <a href=\"{LEARN_MORE_URL}\">Learn More\u{2026}</a>"
    ));
    subtitle.set_halign(gtk::Align::Start);
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("settings-row-subtitle");
    subtitle.add_css_class("wifi-subtitle");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.append(&title);
    text.append(&subtitle);

    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Start);

    top.append(&icon);
    top.append(&text);
    top.append(&switch);
    card.append(&top);

    let connected = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.append(&connected);
    body.append(&card);

    // ---- lists ----
    let lists = gtk::Box::new(gtk::Orientation::Vertical, 8);
    lists.set_margin_top(10);
    body.append(&lists);

    let slot: Slot = Rc::new(RefCell::new(None));
    let ui = Ui {
        switch: switch.clone(),
        connected,
        lists,
        updating: Rc::new(Cell::new(false)),
        slot: Rc::clone(&slot),
    };

    let refresh: Rc<dyn Fn()> = {
        let ui = ui.clone();
        Rc::new(move || {
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                if let Ok(state) = gtk::gio::spawn_blocking(read_state).await {
                    apply(&ui, &state);
                }
            });
        })
    };
    *slot.borrow_mut() = Some(refresh);

    {
        let ui = ui.clone();
        switch.connect_active_notify(move |switch| {
            if ui.updating.get() {
                return;
            }
            let on = switch.is_active();
            // Show the result straight away; the refresh below confirms it.
            ui.connected.set_visible(on);
            ui.lists.set_visible(on);
            let slot = Rc::clone(&ui.slot);
            run_nmcli(strings(&["radio", "wifi", if on { "on" } else { "off" }]), move |_| {
                refresh_later(&slot, 800);
                if on {
                    rescan(&slot);
                }
            });
        });
    }

    // Look again every time the page is shown, and while it stays open.
    {
        let slot = Rc::clone(&slot);
        body.connect_map(move |_| {
            refresh_now(&slot);
            rescan(&slot);
        });
    }
    {
        let slot = Rc::clone(&slot);
        let weak = body.downgrade();
        glib::timeout_add_seconds_local(10, move || {
            let Some(body) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if body.is_mapped() {
                refresh_now(&slot);
            }
            glib::ControlFlow::Continue
        });
    }
    // The refresh closure holds the page's widgets and the slot holds the
    // closure: break that loop when the page goes away.
    body.connect_destroy(move |_| {
        slot.borrow_mut().take();
    });
}
