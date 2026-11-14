//! The Bluetooth page of System Settings, backed by `bluetoothctl` (BlueZ).

use std::cell::{Cell, RefCell};
use std::process::{Command, Stdio};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::context_menu::ContextMenu;
use crate::settings::section_heading;
use crate::wifi_page::{
    clear, group, label, more_glyph, refresh_later, refresh_now, separator, strings, Slot,
};

const LEARN_MORE_URL: &str = "https://www.bluez.org/";

#[derive(Clone)]
struct Device {
    address: String,
    name: String,
    paired: bool,
    connected: bool,
    icon: &'static str,
}

struct State {
    available: bool,
    powered: bool,
    alias: String,
    devices: Vec<Device>,
}

fn ctl(args: &[&str]) -> Option<String> {
    let output = Command::new("bluetoothctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `Device AA:BB:CC:DD:EE:FF Name` lines.
fn device_lines(args: &[&str]) -> Vec<(String, String)> {
    ctl(args)
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let rest = line.trim().strip_prefix("Device ")?;
                    let (address, name) = rest.split_once(' ')?;
                    Some((address.to_string(), name.trim().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn icon_for(kind: &str, name: &str) -> &'static str {
    let kind = kind.to_lowercase();
    let name = name.to_lowercase();
    if kind.contains("headset") || kind.contains("headphone") || kind.contains("audio") {
        "audio-headphones-symbolic"
    } else if kind.contains("keyboard") {
        "input-keyboard-symbolic"
    } else if kind.contains("mouse") {
        "input-mouse-symbolic"
    } else if kind.contains("gaming") {
        "input-gaming-symbolic"
    } else if kind.contains("phone") {
        "phone-symbolic"
    } else if kind.contains("computer") {
        "computer-symbolic"
    } else if kind.contains("video") || kind.contains("display") || name.contains("tv") {
        "video-display-symbolic"
    } else {
        "bluetooth-symbolic"
    }
}

fn read_state() -> State {
    let Some(show) = ctl(&["show"]).filter(|text| text.contains("Powered:")) else {
        return State { available: false, powered: false, alias: String::new(), devices: Vec::new() };
    };
    let field = |key: &str| {
        show.lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .map(|value| value.trim().to_string())
    };
    let powered = field("Powered:").as_deref() == Some("yes");
    let alias = field("Alias:").unwrap_or_else(|| "this computer".into());

    let mut devices = Vec::new();
    if powered {
        let paired = device_lines(&["devices", "Paired"]);
        let paired = if paired.is_empty() { device_lines(&["paired-devices"]) } else { paired };
        let connected = device_lines(&["devices", "Connected"]);
        for (address, name) in device_lines(&["devices"]) {
            let kind = ctl(&["info", &address])
                .and_then(|info| {
                    info.lines()
                        .find_map(|line| line.trim().strip_prefix("Icon:").map(|v| v.trim().to_string()))
                })
                .unwrap_or_default();
            // Unnamed devices show up as their address: not worth listing.
            if name.replace('-', ":") == address {
                continue;
            }
            devices.push(Device {
                paired: paired.iter().any(|(a, _)| *a == address),
                connected: connected.iter().any(|(a, _)| *a == address),
                icon: icon_for(&kind, &name),
                address,
                name,
            });
        }
        devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
    }
    State { available: true, powered, alias, devices }
}

fn run_ctl(args: Vec<String>, done: impl FnOnce(bool) + 'static) {
    glib::spawn_future_local(async move {
        let ok = gtk::gio::spawn_blocking(move || {
            Command::new("bluetoothctl")
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

#[derive(Clone)]
struct Ui {
    switch: gtk::Switch,
    discoverable: gtk::Label,
    body: gtk::Box,
    mine: gtk::Box,
    nearby: gtk::Box,
    spinner: gtk::Spinner,
    updating: Rc<Cell<bool>>,
    slot: Slot,
}

fn device_image(icon: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(20);
    image.add_css_class("bt-device-icon");
    image
}

fn connect(device: &Device, slot: &Slot) {
    let slot = Rc::clone(slot);
    let address = device.address.clone();
    let needs_pairing = !device.paired;
    glib::spawn_future_local(async move {
        let _ = gtk::gio::spawn_blocking(move || {
            if needs_pairing {
                for step in ["trust", "pair"] {
                    let _ = Command::new("bluetoothctl")
                        .args([step, &address])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
            let _ = Command::new("bluetoothctl")
                .args(["connect", &address])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        })
        .await;
        refresh_later(&slot, 300);
    });
}

fn simple(args: &[&str], slot: &Slot) {
    let slot = Rc::clone(slot);
    run_ctl(strings(args), move |_| refresh_later(&slot, 400));
}

fn more_button(device: &Device, slot: &Slot) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("wifi-more");
    button.set_valign(gtk::Align::Center);
    button.set_child(Some(&more_glyph()));
    let menu = ContextMenu::new(gtk::PositionType::Bottom, &["settings-popup-menu"]);
    menu.set_parent(&button);
    if device.connected {
        let (address, slot) = (device.address.clone(), Rc::clone(slot));
        menu.append_action("Disconnect", true, move || simple(&["disconnect", &address], &slot));
    } else {
        let (device, slot) = (device.clone(), Rc::clone(slot));
        menu.append_action("Connect", true, move || connect(&device, &slot));
    }
    let (address, slot) = (device.address.clone(), Rc::clone(slot));
    menu.append_action("Forget This Device\u{2026}", true, move || simple(&["remove", &address], &slot));
    let popup = menu.clone();
    button.connect_clicked(move |button| popup.popup_at(button.width() / 2, button.height()));
    button.connect_destroy(move |_| menu.unparent());
    button
}

fn device_row(device: &Device, slot: &Slot, with_menu: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("settings-row");
    row.add_css_class("bt-row");
    row.append(&device_image(device.icon));
    let name = label(&device.name, &["settings-row-title"]);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&name);
    if with_menu {
        let status = label(if device.connected { "Connected" } else { "Not Connected" }, &["wifi-status-text"]);
        status.set_margin_end(4);
        row.append(&status);
        row.append(&more_button(device, slot));
    }
    if !device.connected {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let (device, slot) = (device.clone(), Rc::clone(slot));
        click.connect_released(move |_, _, _, _| connect(&device, &slot));
        row.add_controller(click);
    }
    row
}

fn card(rows: &[gtk::Box]) -> gtk::Box {
    group(rows)
}

fn apply(ui: &Ui, state: &State) {
    ui.updating.set(true);
    ui.switch.set_active(state.powered);
    ui.switch.set_sensitive(state.available);
    ui.updating.set(false);
    ui.discoverable.set_text(&format!(
        "This Mac is discoverable as \u{201c}{}\u{201d} while Bluetooth Settings is open.",
        state.alias
    ));
    ui.discoverable.set_visible(state.powered);
    ui.body.set_visible(state.powered || !state.available);

    clear(&ui.mine);
    clear(&ui.nearby);
    if !state.available {
        ui.mine.append(&label(
            "Bluetooth is unavailable: BlueZ could not be reached.",
            &["settings-row-subtitle"],
        ));
        return;
    }
    let mine: Vec<&Device> = state.devices.iter().filter(|d| d.paired).collect();
    let nearby: Vec<&Device> = state.devices.iter().filter(|d| !d.paired).collect();

    if mine.is_empty() {
        let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        empty.add_css_class("settings-row");
        empty.add_css_class("bt-empty-row");
        let text = label("No Bluetooth Devices", &["bt-empty"]);
        text.set_hexpand(true);
        text.set_halign(gtk::Align::Center);
        text.set_xalign(0.5);
        empty.append(&text);
        ui.mine.append(&card(&[empty]));
    } else {
        let rows: Vec<gtk::Box> = mine.iter().map(|d| device_row(d, &ui.slot, true)).collect();
        ui.mine.append(&card(&rows));
    }
    if !nearby.is_empty() {
        let rows: Vec<gtk::Box> = nearby.iter().map(|d| device_row(d, &ui.slot, false)).collect();
        ui.nearby.append(&card(&rows));
    }
}

/// Scans for a few seconds, refreshing as devices appear.
fn scan(ui: &Ui) {
    ui.spinner.set_spinning(true);
    ui.spinner.set_visible(true);
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let _ = gtk::gio::spawn_blocking(|| {
            let _ = Command::new("bluetoothctl")
                .args(["--timeout", "8", "scan", "on"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        })
        .await;
        ui.spinner.set_spinning(false);
        refresh_now(&ui.slot);
    });
}

pub fn build(body: &gtk::Box) {
    // ---- top card ----
    let card_top = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card_top.add_css_class("settings-group");
    card_top.add_css_class("wifi-group");

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.add_css_class("settings-row");
    top.add_css_class("wifi-header-row");
    let icon = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../assets/settings-icons/bluetooth.png"
    ))) {
        icon.set_paintable(Some(&texture));
    }
    icon.set_pixel_size(26);
    icon.set_valign(gtk::Align::Start);
    let subtitle = gtk::Label::new(None);
    subtitle.set_markup(&format!(
        "Connect to accessories you can use for activities such as streaming music, typing and gaming. <a href=\"{LEARN_MORE_URL}\">Learn more\u{2026}</a>"
    ));
    subtitle.set_halign(gtk::Align::Start);
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("settings-row-subtitle");
    subtitle.add_css_class("wifi-subtitle");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.append(&label("Bluetooth", &["settings-row-title"]));
    text.append(&subtitle);
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Start);
    top.append(&icon);
    top.append(&text);
    top.append(&switch);
    card_top.append(&top);
    card_top.append(&separator());
    let discoverable = label("", &["settings-row-subtitle", "bt-discoverable"]);
    card_top.append(&discoverable);
    body.append(&card_top);

    // ---- device lists ----
    let lists = gtk::Box::new(gtk::Orientation::Vertical, 8);
    lists.set_margin_top(10);
    body.append(&lists);

    lists.append(&section_heading("My Devices"));
    let mine = gtk::Box::new(gtk::Orientation::Vertical, 0);
    lists.append(&mine);

    let help = gtk::Button::with_label("?");
    help.add_css_class("bt-help");
    help.set_halign(gtk::Align::End);
    help.set_margin_top(6);
    help.connect_clicked(|_| {
        let _ = gtk::gio::AppInfo::launch_default_for_uri(LEARN_MORE_URL, None::<&gtk::gio::AppLaunchContext>);
    });
    lists.append(&help);

    let heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    heading_row.set_margin_top(14);
    let heading = section_heading("Nearby Devices");
    heading.set_hexpand(true);
    let spinner = gtk::Spinner::new();
    spinner.set_margin_end(10);
    spinner.add_css_class("bt-spinner");
    heading_row.append(&heading);
    heading_row.append(&spinner);
    lists.append(&heading_row);
    let nearby = gtk::Box::new(gtk::Orientation::Vertical, 0);
    lists.append(&nearby);

    let slot: Slot = Rc::new(RefCell::new(None));
    let ui = Ui {
        switch: switch.clone(),
        discoverable,
        body: lists,
        mine,
        nearby,
        spinner,
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
            ui.body.set_visible(on);
            ui.discoverable.set_visible(on);
            let (slot, ui) = (Rc::clone(&ui.slot), ui.clone());
            run_ctl(strings(&["power", if on { "on" } else { "off" }]), move |_| {
                refresh_later(&slot, 600);
                if on {
                    scan(&ui);
                }
            });
        });
    }
    {
        let ui = ui.clone();
        body.connect_map(move |_| {
            refresh_now(&ui.slot);
            scan(&ui);
        });
    }
    {
        let slot = Rc::clone(&slot);
        let weak = body.downgrade();
        glib::timeout_add_seconds_local(8, move || {
            let Some(body) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if body.is_mapped() {
                refresh_now(&slot);
            }
            glib::ControlFlow::Continue
        });
    }
    body.connect_destroy(move |_| {
        slot.borrow_mut().take();
    });
}
