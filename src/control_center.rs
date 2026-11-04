//! Control Centre: the menu-bar button between Wi-Fi and the clock, and the
//! panel of quick toggles, sliders and media controls it opens.
//!
//! Every control talks to a small command-line helper (`nmcli`, `bluetoothctl`,
//! `wpctl`, `brightnessctl`, `playerctl`, `makoctl`/`dunstctl`). A helper that
//! is not installed simply leaves its control inactive. State is read when the
//! panel opens and again shortly after each action.

use std::cell::Cell;
use std::ffi::OsStr;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, glib, prelude::*};

use crate::context_menu;

/// Width of one grid cell, and the gap between cells, in pixels.
const CELL: i32 = 64;
const GAP: i32 = 10;

// ---------------------------------------------------------------- commands

/// Runs a helper and returns its trimmed stdout, or `None` if it is missing or
/// fails. Blocking: call it off the main thread.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program).args(args).output().ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_string())
}

/// Runs a helper in the background, then calls `then` on the main thread.
fn run_then(program: &'static str, args: Vec<String>, then: Rc<dyn Fn()>) {
    glib::spawn_future_local(async move {
        let _ = gtk::gio::spawn_blocking(move || Command::new(program).args(&args).status()).await;
        then();
    });
}

/// Runs a helper in the background and ignores the result.
fn fire(program: &'static str, args: Vec<String>) {
    std::thread::spawn(move || {
        let _ = Command::new(program).args(&args).status();
    });
}

/// Starts the first installed program of `programs`, detached from the shell.
fn launch_first(programs: &[&str]) {
    for program in programs {
        if crate::dock::spawn_detached(OsStr::new(program), &[]).is_ok() {
            return;
        }
    }
    eprintln!("lunar-desktop: none of {programs:?} is installed");
}

/// Asks KWin to run one of its global shortcuts (Overview, Show Desktop, ...).
fn kwin_shortcut(name: &'static str) {
    std::thread::spawn(move || {
        for qdbus in ["qdbus6", "qdbus"] {
            let done = Command::new(qdbus)
                .args([
                    "org.kde.kglobalaccel",
                    "/component/kwin",
                    "org.kde.kglobalaccel.Component.invokeShortcut",
                    name,
                ])
                .status()
                .is_ok_and(|status| status.success());
            if done {
                return;
            }
        }
        eprintln!("lunar-desktop: could not run the KWin shortcut {name}");
    });
}

// ------------------------------------------------------------------- state

#[derive(Default)]
struct Media {
    playing: bool,
    title: String,
    artist: String,
    art: Option<String>,
}

#[derive(Default)]
struct State {
    wifi_available: bool,
    wifi_on: bool,
    ssid: Option<String>,
    bluetooth_available: bool,
    bluetooth_on: bool,
    do_not_disturb: bool,
    volume: Option<f64>,
    muted: bool,
    brightness: Option<f64>,
    media: Option<Media>,
}

/// Reads every control's current state. Blocking: run it off the main thread.
fn read_state() -> State {
    let mut state = State::default();

    if let Some(radio) = output("nmcli", &["-t", "-f", "WIFI", "radio"]) {
        state.wifi_available = true;
        state.wifi_on = radio == "enabled";
        if state.wifi_on {
            state.ssid = output("nmcli", &["-t", "-f", "ACTIVE,SSID", "device", "wifi"])
                .and_then(|list| {
                    list.lines()
                        .find_map(|line| line.strip_prefix("yes:").map(str::to_string))
                })
                .map(|ssid| ssid.replace("\\:", ":"))
                .filter(|ssid| !ssid.is_empty());
        }
    }

    if let Some(show) = output("bluetoothctl", &["show"]) {
        state.bluetooth_available = true;
        state.bluetooth_on = show.contains("Powered: yes");
    }

    state.do_not_disturb = if let Some(mode) = output("makoctl", &["mode"]) {
        mode.lines().any(|line| line.trim() == "do-not-disturb")
    } else {
        output("dunstctl", &["is-paused"]).is_some_and(|paused| paused == "true")
    };

    if let Some(volume) = output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
        // "Volume: 0.45" or "Volume: 0.45 [MUTED]".
        state.muted = volume.contains("[MUTED]");
        state.volume = volume
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<f64>().ok());
    }

    if let Some(info) = output("brightnessctl", &["-m"]) {
        // "device,class,current,45%,max"
        state.brightness = info
            .split(',')
            .nth(3)
            .and_then(|percent| percent.trim_end_matches('%').parse::<f64>().ok())
            .map(|percent| percent / 100.0);
    }

    if let Some(line) = output(
        "playerctl",
        &[
            "metadata",
            "--format",
            "{{status}}|{{title}}|{{artist}}|{{mpris:artUrl}}",
        ],
    ) {
        let mut parts = line.splitn(4, '|');
        let status = parts.next().unwrap_or_default();
        let title = parts.next().unwrap_or_default().to_string();
        let artist = parts.next().unwrap_or_default().to_string();
        let art = parts
            .next()
            .map(str::to_string)
            .filter(|url| !url.is_empty());
        if !title.is_empty() {
            state.media = Some(Media {
                playing: status == "Playing",
                title,
                artist,
                art,
            });
        }
    }

    state
}

// ----------------------------------------------------------------- widgets

fn icon(names: &[&str], size: i32) -> gtk::Image {
    let theme = gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display));
    let name = theme
        .and_then(|theme| names.iter().copied().find(|name| theme.has_icon(name)))
        .or_else(|| names.first().copied())
        .unwrap_or("image-missing");
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(size);
    image.add_css_class("cc-icon");
    image
}

fn set_on(widget: &impl IsA<gtk::Widget>, on: bool) {
    if on {
        widget.add_css_class("on");
    } else {
        widget.remove_css_class("on");
    }
}

fn is_on(widget: &impl IsA<gtk::Widget>) -> bool {
    widget.has_css_class("on")
}

/// A round (or square) toggle button holding one icon.
fn round_button(names: &[&str], extra_class: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&icon(names, 18)));
    button.add_css_class("cc-button");
    button.add_css_class(extra_class);
    button.set_size_request(CELL, CELL);
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button
}

/// A wide pill with an icon disc, a title and an optional second line.
fn pill_button(names: &[&str], title: &str, subtitle: Option<&str>) -> (gtk::Button, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_valign(gtk::Align::Center);
    let disc = gtk::Box::new(gtk::Orientation::Vertical, 0);
    disc.add_css_class("cc-disc");
    disc.set_size_request(34, 34);
    disc.set_valign(gtk::Align::Center);
    let glyph = icon(names, 16);
    glyph.set_halign(gtk::Align::Center);
    glyph.set_valign(gtk::Align::Center);
    glyph.set_vexpand(true);
    disc.append(&glyph);
    row.append(&disc);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_valign(gtk::Align::Center);
    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("cc-title");
    title_label.set_xalign(0.0);
    text.append(&title_label);
    let sub = gtk::Label::new(subtitle);
    sub.add_css_class("cc-subtitle");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.set_max_width_chars(12);
    sub.set_visible(subtitle.is_some());
    text.append(&sub);
    row.append(&text);

    let button = gtk::Button::new();
    button.set_child(Some(&row));
    button.add_css_class("cc-button");
    button.add_css_class("cc-pill");
    button.set_size_request(CELL * 2 + GAP, CELL);
    (button, sub)
}

/// Collapses rapid slider movement into one helper call every 100 ms.
fn debounce(apply: impl Fn(f64) + 'static) -> Rc<dyn Fn(f64)> {
    let latest = Rc::new(Cell::new(0.0_f64));
    let scheduled = Rc::new(Cell::new(false));
    let apply = Rc::new(apply);
    Rc::new(move |value| {
        latest.set(value);
        if scheduled.replace(true) {
            return;
        }
        let latest = Rc::clone(&latest);
        let scheduled = Rc::clone(&scheduled);
        let apply = Rc::clone(&apply);
        glib::timeout_add_local_once(Duration::from_millis(100), move || {
            scheduled.set(false);
            apply(latest.get());
        });
    })
}

/// A titled slider tile with a small icon at each end (or a button at the end).
fn slider_tile(
    title: &str,
    start_icon: &[&str],
    end_widget: gtk::Widget,
) -> (gtk::Box, gtk::Scale) {
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
    tile.add_css_class("cc-tile");
    tile.set_size_request(CELL * 4 + GAP * 3, 58);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("cc-title");
    heading.set_xalign(0.0);
    heading.set_margin_start(14);
    heading.set_margin_top(7);
    tile.append(&heading);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_start(12);
    row.set_margin_end(8);
    row.set_margin_bottom(6);
    row.append(&icon(start_icon, 12));
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_valign(gtk::Align::Center);
    scale.add_css_class("cc-slider");
    row.append(&scale);
    row.append(&end_widget);
    tile.append(&row);
    (tile, scale)
}

// ------------------------------------------------------------- bar button

/// The two-switches glyph shown in the menu bar.
fn bar_glyph() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(14);
    area.set_valign(gtk::Align::Center);
    area.add_css_class("lunar-menubar-item");
    area.set_draw_func(|area, cr, width, _height| {
        let color = area.color();
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            color.alpha() as f64,
        );
        let width = width as f64;
        let radius = 3.1;
        let pill = |y: f64, knob_right: bool| {
            let (x0, x1) = (1.0, width - 1.0);
            cr.new_sub_path();
            cr.arc(x1 - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
            cr.arc(x0 + radius, y + radius, radius, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
            cr.close_path();
            cr.set_line_width(1.1);
            let _ = cr.stroke();
            let knob_x = if knob_right { x1 - radius } else { x0 + radius };
            cr.arc(knob_x, y + radius, radius - 1.6, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
        };
        pill(1.0, true);
        pill(7.6, false);
    });

    // The bar's text colour changes with the wallpaper; repaint when it does.
    let last = Cell::new([-1.0_f32; 4]);
    let weak = area.downgrade();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(area) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let color = area.color();
        let rgba = [color.red(), color.green(), color.blue(), color.alpha()];
        if rgba != last.get() {
            last.set(rgba);
            area.queue_draw();
        }
        glib::ControlFlow::Continue
    });
    area
}

/// Builds the Control Centre button for the menu bar.
pub fn control_center_button(app: &gtk::Application) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_offset(0, 2);
    popover.add_css_class("lunar-menu-popover");
    popover.set_autohide(true);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("lunar-system-menu");
    panel.add_css_class("cc-panel");

    let grid = gtk::Grid::new();
    grid.set_column_spacing(GAP as u32);
    grid.set_row_spacing(GAP as u32);
    grid.set_column_homogeneous(true);
    panel.append(&grid);

    // ---- Wi-Fi, Bluetooth, Airplane ----------------------------------------
    let (wifi, wifi_sub) = pill_button(
        &["network-wireless-symbolic", "network-wireless"],
        "Wi-Fi",
        Some("Off"),
    );
    grid.attach(&wifi, 0, 0, 2, 1);

    let bluetooth = round_button(&["bluetooth-symbolic", "bluetooth-active-symbolic"], "cc-circle");
    grid.attach(&bluetooth, 0, 1, 1, 1);
    let airplane = round_button(
        &["airplane-mode-symbolic", "network-wireless-disabled-symbolic"],
        "cc-circle",
    );
    grid.attach(&airplane, 1, 1, 1, 1);

    // ---- Media -------------------------------------------------------------
    let media = gtk::Box::new(gtk::Orientation::Vertical, 4);
    media.add_css_class("cc-tile");
    media.set_size_request(CELL * 2 + GAP, CELL * 2 + GAP);
    let art_overlay = gtk::Overlay::new();
    art_overlay.add_css_class("cc-art");
    art_overlay.set_size_request(40, 40);
    art_overlay.set_halign(gtk::Align::Start);
    art_overlay.set_margin_start(12);
    art_overlay.set_margin_top(12);
    let art_fallback = icon(&["audio-x-generic-symbolic", "multimedia-player-symbolic"], 20);
    art_fallback.set_halign(gtk::Align::Center);
    art_fallback.set_valign(gtk::Align::Center);
    art_overlay.set_child(Some(&art_fallback));
    let art = gtk::Picture::new();
    art.set_content_fit(gtk::ContentFit::Cover);
    art.set_visible(false);
    art_overlay.add_overlay(&art);
    media.append(&art_overlay);

    let track_title = gtk::Label::new(Some("Not Playing"));
    track_title.add_css_class("cc-title");
    track_title.set_xalign(0.0);
    track_title.set_margin_start(12);
    track_title.set_margin_end(10);
    track_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    track_title.set_max_width_chars(14);
    media.append(&track_title);
    let track_artist = gtk::Label::new(None);
    track_artist.add_css_class("cc-subtitle");
    track_artist.set_xalign(0.0);
    track_artist.set_margin_start(12);
    track_artist.set_margin_end(10);
    track_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    track_artist.set_max_width_chars(14);
    media.append(&track_artist);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    transport.set_halign(gtk::Align::Center);
    transport.set_valign(gtk::Align::End);
    transport.set_vexpand(true);
    transport.set_margin_bottom(8);
    let previous = gtk::Button::new();
    previous.set_child(Some(&icon(&["media-skip-backward-symbolic"], 16)));
    let play = gtk::Button::new();
    let play_icon = icon(&["media-playback-start-symbolic"], 18);
    play.set_child(Some(&play_icon));
    let next = gtk::Button::new();
    next.set_child(Some(&icon(&["media-skip-forward-symbolic"], 16)));
    for button in [&previous, &play, &next] {
        button.add_css_class("cc-transport");
    }
    transport.append(&previous);
    transport.append(&play);
    transport.append(&next);
    media.append(&transport);
    grid.attach(&media, 2, 0, 2, 2);

    // ---- Do Not Disturb and window shortcuts ---------------------------------
    let (focus, focus_sub) = pill_button(
        &["notifications-disabled-symbolic", "weather-clear-night-symbolic"],
        "Focus",
        None,
    );
    grid.attach(&focus, 0, 2, 2, 1);
    let overview = round_button(
        &["view-app-grid-symbolic", "view-grid-symbolic", "window-duplicate-symbolic"],
        "cc-square",
    );
    overview.set_tooltip_text(Some("Overview"));
    grid.attach(&overview, 2, 2, 1, 1);
    let desktop = round_button(
        &["user-desktop-symbolic", "window-restore-symbolic"],
        "cc-square",
    );
    desktop.set_tooltip_text(Some("Show Desktop"));
    grid.attach(&desktop, 3, 2, 1, 1);

    // ---- Display and Sound sliders --------------------------------------------
    let (display_tile, display_scale) = slider_tile(
        "Display",
        &["display-brightness-low-symbolic", "weather-clear-symbolic"],
        icon(&["display-brightness-symbolic", "weather-clear-symbolic"], 16).upcast(),
    );
    grid.attach(&display_tile, 0, 3, 4, 1);

    let mute = gtk::Button::new();
    mute.set_child(Some(&icon(
        &["audio-volume-high-symbolic", "audio-speakers-symbolic"],
        14,
    )));
    mute.add_css_class("cc-mini-circle");
    let (sound_tile, sound_scale) = slider_tile(
        "Sound",
        &["audio-volume-low-symbolic", "audio-volume-medium-symbolic"],
        mute.clone().upcast(),
    );
    grid.attach(&sound_tile, 0, 4, 4, 1);

    // ---- Bottom row of shortcuts ------------------------------------------------
    let appearance = round_button(
        &["weather-clear-night-symbolic", "preferences-desktop-theme-symbolic"],
        "cc-circle",
    );
    appearance.set_tooltip_text(Some("Dark Mode"));
    grid.attach(&appearance, 0, 5, 1, 1);
    let calculator = round_button(
        &["accessories-calculator-symbolic", "kcalc-symbolic"],
        "cc-circle",
    );
    calculator.set_tooltip_text(Some("Calculator"));
    grid.attach(&calculator, 1, 5, 1, 1);
    let timer = round_button(
        &["alarm-symbolic", "preferences-system-time-symbolic", "appointment-soon-symbolic"],
        "cc-circle",
    );
    timer.set_tooltip_text(Some("Clock"));
    grid.attach(&timer, 2, 5, 1, 1);
    let capture = round_button(
        &["applets-screenshooter-symbolic", "camera-photo-symbolic"],
        "cc-circle",
    );
    capture.set_tooltip_text(Some("Screenshot"));
    grid.attach(&capture, 3, 5, 1, 1);

    let edit = gtk::Button::with_label("Edit Controls");
    edit.add_css_class("cc-edit");
    edit.set_halign(gtk::Align::Center);
    edit.set_margin_top(8);
    panel.append(&edit);

    popover.set_child(Some(&panel));
    // The same compositor blur the other menus use, shaped to the panel.
    // Only the tiles are blurred, not the panel behind and between them.
    let blur_regions: Vec<(gtk::Widget, i32)> = vec![
        (wifi.clone().upcast(), 22),
        (media.clone().upcast(), 22),
        (bluetooth.clone().upcast(), 32),
        (airplane.clone().upcast(), 32),
        (focus.clone().upcast(), 22),
        (overview.clone().upcast(), 20),
        (desktop.clone().upcast(), 20),
        (display_tile.clone().upcast(), 22),
        (sound_tile.clone().upcast(), 22),
        (appearance.clone().upcast(), 32),
        (calculator.clone().upcast(), 32),
        (timer.clone().upcast(), 32),
        (capture.clone().upcast(), 32),
        (edit.clone().upcast(), 14),
    ];
    let _refresh_blur = context_menu::attach_blur_regions(&popover, blur_regions);

    // ---- Refreshing from the system ----------------------------------------------
    // True while a widget is being set from the system, so the change is not
    // mistaken for the user moving it.
    let updating = Rc::new(Cell::new(false));

    let refresh: Rc<dyn Fn()> = {
        let wifi = wifi.clone();
        let wifi_sub = wifi_sub.clone();
        let bluetooth = bluetooth.clone();
        let airplane = airplane.clone();
        let focus = focus.clone();
        let focus_sub = focus_sub.clone();
        let appearance = appearance.clone();
        let display_scale = display_scale.clone();
        let sound_scale = sound_scale.clone();
        let mute = mute.clone();
        let media_box = media.clone();
        let track_title = track_title.clone();
        let track_artist = track_artist.clone();
        let play_icon = play_icon.clone();
        let art = art.clone();
        let updating = Rc::clone(&updating);
        Rc::new(move || {
            let wifi = wifi.clone();
            let wifi_sub = wifi_sub.clone();
            let bluetooth = bluetooth.clone();
            let airplane = airplane.clone();
            let focus = focus.clone();
            let focus_sub = focus_sub.clone();
            let appearance = appearance.clone();
            let display_scale = display_scale.clone();
            let sound_scale = sound_scale.clone();
            let mute = mute.clone();
            let media_box = media_box.clone();
            let track_title = track_title.clone();
            let track_artist = track_artist.clone();
            let play_icon = play_icon.clone();
            let art = art.clone();
            let updating = Rc::clone(&updating);
            glib::spawn_future_local(async move {
                let Ok(state) = gtk::gio::spawn_blocking(read_state).await else {
                    return;
                };
                updating.set(true);

                wifi.set_sensitive(state.wifi_available);
                set_on(&wifi, state.wifi_on);
                wifi_sub.set_visible(true);
                wifi_sub.set_text(if !state.wifi_on {
                    "Off"
                } else {
                    state.ssid.as_deref().unwrap_or("Not Connected")
                });

                bluetooth.set_sensitive(state.bluetooth_available);
                set_on(&bluetooth, state.bluetooth_on);
                set_on(&airplane, !state.wifi_on && !state.bluetooth_on);

                set_on(&focus, state.do_not_disturb);
                focus_sub.set_visible(state.do_not_disturb);
                focus_sub.set_text("Do Not Disturb");
                set_on(&appearance, crate::preferences::current().dark_mode);

                display_scale.set_sensitive(state.brightness.is_some());
                if let Some(value) = state.brightness {
                    display_scale.set_value(value);
                }
                sound_scale.set_sensitive(state.volume.is_some());
                if let Some(value) = state.volume {
                    sound_scale.set_value(value.min(1.0));
                }
                set_on(&mute, state.muted);

                let playing = state.media.as_ref().is_some_and(|media| media.playing);
                play_icon.set_icon_name(Some(if playing {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                }));
                media_box.set_sensitive(state.media.is_some());
                match &state.media {
                    Some(media) => {
                        track_title.set_text(&media.title);
                        track_artist.set_text(&media.artist);
                        let texture = media
                            .art
                            .as_deref()
                            .and_then(|url| url.strip_prefix("file://"))
                            .and_then(|path| gdk::Texture::from_filename(path).ok());
                        art.set_paintable(texture.as_ref());
                        art.set_visible(texture.is_some());
                    }
                    None => {
                        track_title.set_text("Not Playing");
                        track_artist.set_text("");
                        art.set_paintable(None::<&gdk::Paintable>);
                        art.set_visible(false);
                    }
                }

                updating.set(false);
            });
        })
    };
    {
        let refresh = Rc::clone(&refresh);
        popover.connect_show(move |_| refresh());
    }

    // Re-reads the system a moment after an action, once it has taken effect.
    let refresh_soon: Rc<dyn Fn()> = {
        let refresh = Rc::clone(&refresh);
        Rc::new(move || {
            let refresh = Rc::clone(&refresh);
            glib::timeout_add_local_once(Duration::from_millis(700), move || refresh());
        })
    };

    // ---- Actions --------------------------------------------------------------------
    {
        let refresh_soon = Rc::clone(&refresh_soon);
        wifi.connect_clicked(move |button| {
            let turn_on = !is_on(button);
            set_on(button, turn_on);
            let arg = if turn_on { "on" } else { "off" };
            run_then("nmcli", vec!["radio".into(), "wifi".into(), arg.into()], Rc::clone(&refresh_soon));
        });
    }
    {
        let refresh_soon = Rc::clone(&refresh_soon);
        bluetooth.connect_clicked(move |button| {
            let turn_on = !is_on(button);
            set_on(button, turn_on);
            let arg = if turn_on { "on" } else { "off" };
            run_then("bluetoothctl", vec!["power".into(), arg.into()], Rc::clone(&refresh_soon));
        });
    }
    {
        let refresh_soon = Rc::clone(&refresh_soon);
        airplane.connect_clicked(move |button| {
            let radios_on = is_on(button);
            set_on(button, !radios_on);
            let arg = if radios_on { "on" } else { "off" };
            fire("bluetoothctl", vec!["power".into(), arg.into()]);
            run_then("nmcli", vec!["radio".into(), "wifi".into(), arg.into()], Rc::clone(&refresh_soon));
        });
    }
    {
        let refresh_soon = Rc::clone(&refresh_soon);
        focus.connect_clicked(move |button| {
            let enable = !is_on(button);
            set_on(button, enable);
            let refresh_soon = Rc::clone(&refresh_soon);
            glib::spawn_future_local(async move {
                let _ = gtk::gio::spawn_blocking(move || {
                    // mako first; dunst if mako is not running.
                    let mako = if enable {
                        Command::new("makoctl").args(["mode", "-a", "do-not-disturb"]).status()
                    } else {
                        Command::new("makoctl").args(["mode", "-r", "do-not-disturb"]).status()
                    };
                    if !mako.is_ok_and(|status| status.success()) {
                        let _ = Command::new("dunstctl")
                            .args(["set-paused", if enable { "true" } else { "false" }])
                            .status();
                    }
                })
                .await;
                refresh_soon();
            });
        });
    }
    overview.connect_clicked({
        let popover = popover.downgrade();
        move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            kwin_shortcut("Overview");
        }
    });
    desktop.connect_clicked({
        let popover = popover.downgrade();
        move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            kwin_shortcut("Show Desktop");
        }
    });

    {
        let updating = Rc::clone(&updating);
        let apply = debounce(|value| {
            fire("brightnessctl", vec!["set".into(), format!("{}%", (value * 100.0).round() as i32)]);
        });
        display_scale.connect_value_changed(move |scale| {
            if !updating.get() {
                apply(scale.value());
            }
        });
    }
    {
        let updating = Rc::clone(&updating);
        let apply = debounce(|value| {
            fire(
                "wpctl",
                vec!["set-volume".into(), "@DEFAULT_AUDIO_SINK@".into(), format!("{value:.2}")],
            );
        });
        sound_scale.connect_value_changed(move |scale| {
            if !updating.get() {
                apply(scale.value());
            }
        });
    }
    {
        let refresh_soon = Rc::clone(&refresh_soon);
        mute.connect_clicked(move |button| {
            set_on(button, !is_on(button));
            run_then(
                "wpctl",
                vec!["set-mute".into(), "@DEFAULT_AUDIO_SINK@".into(), "toggle".into()],
                Rc::clone(&refresh_soon),
            );
        });
    }

    for (button, command) in [
        (&previous, "previous"),
        (&play, "play-pause"),
        (&next, "next"),
    ] {
        let refresh_soon = Rc::clone(&refresh_soon);
        button.connect_clicked(move |_| {
            run_then("playerctl", vec![command.into()], Rc::clone(&refresh_soon));
        });
    }

    {
        let app = app.clone();
        let refresh_soon = Rc::clone(&refresh_soon);
        appearance.connect_clicked(move |button| {
            let dark = !is_on(button);
            set_on(button, dark);
            crate::preferences::set_dark_mode(dark, &app);
            refresh_soon();
        });
    }
    for (button, programs) in [
        (&calculator, &["kcalc", "gnome-calculator", "qalculate-gtk"][..]),
        (&timer, &["kclock", "gnome-clocks"][..]),
        (&capture, &["spectacle", "gnome-screenshot"][..]),
    ] {
        let popover = popover.downgrade();
        let programs = programs.to_vec();
        button.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            launch_first(&programs);
        });
    }
    {
        let app = app.clone();
        let popover = popover.downgrade();
        edit.connect_clicked(move |_| {
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            crate::settings::present(&app);
        });
    }

    // ---- The button in the bar ----------------------------------------------------------
    let button = gtk::MenuButton::new();
    button.set_size_request(30, crate::menubar::HEIGHT);
    button.set_child(Some(&bar_glyph()));
    button.set_popover(Some(&popover));
    button.set_halign(gtk::Align::End);
    button.set_valign(gtk::Align::Center);
    button.add_css_class("lunar-menubar-item-button");
    button.set_can_target(true);
    button
}
