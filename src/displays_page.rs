//! The Displays page of System Settings: resolution (through `kscreen-doctor`),
//! brightness (through `brightnessctl`) and shortcuts to the KDE display
//! settings.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::context_menu::ContextMenu;
use crate::settings::{popup_button, popup_row, settings_group, settings_row, switch_row};
use crate::wifi_page::{clear, glyph, label, refresh_later, refresh_now, separator, Slot};

// ------------------------------------------------------------------- data

#[derive(Clone)]
struct Mode {
    id: String,
    width: i64,
    height: i64,
    refresh: f64,
}

#[derive(Clone)]
struct Output {
    name: String,
    label: String,
    modes: Vec<Mode>,
    current: String,
    preferred: Option<String>,
}

fn read_output() -> Option<Output> {
    let output = Command::new("kscreen-doctor")
        .arg("-j")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let outputs = json.get("outputs")?.as_array()?;
    let chosen = outputs
        .iter()
        .find(|o| o.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false))
        .or_else(|| outputs.first())?;
    let name = chosen.get("name")?.as_str()?.to_string();
    let modes = chosen
        .get("modes")?
        .as_array()?
        .iter()
        .filter_map(|mode| {
            let size = mode.get("size")?;
            Some(Mode {
                id: mode.get("id")?.as_str()?.to_string(),
                width: size.get("width")?.as_i64()?,
                height: size.get("height")?.as_i64()?,
                refresh: mode.get("refreshRate").and_then(|r| r.as_f64()).unwrap_or(0.0),
            })
        })
        .collect::<Vec<_>>();
    if modes.is_empty() {
        return None;
    }
    let builtin = ["eDP", "LVDS", "DSI"].iter().any(|prefix| name.starts_with(prefix));
    Some(Output {
        label: if builtin { "Built-in Display".to_string() } else { name.clone() },
        current: chosen.get("currentModeId").and_then(|m| m.as_str()).unwrap_or_default().to_string(),
        preferred: chosen
            .get("preferredModes")
            .and_then(|m| m.as_array())
            .and_then(|m| m.first())
            .and_then(|m| m.as_str())
            .map(str::to_string),
        name,
        modes,
    })
}

/// One entry per resolution (at its best refresh rate), largest first.
fn resolutions(output: &Output) -> Vec<Mode> {
    let mut sizes: Vec<Mode> = Vec::new();
    for mode in &output.modes {
        match sizes.iter_mut().find(|m| m.width == mode.width && m.height == mode.height) {
            Some(existing) => {
                // Keep the current mode if it is one of this size; else the fastest.
                if existing.id != output.current && (mode.id == output.current || mode.refresh > existing.refresh) {
                    *existing = mode.clone();
                }
            }
            None => sizes.push(mode.clone()),
        }
    }
    sizes.sort_by_key(|m| std::cmp::Reverse(m.width * m.height));
    sizes
}

fn same_size(a: &Mode, b: &Mode) -> bool {
    a.width == b.width && a.height == b.height
}

fn brightness_percent() -> Option<f64> {
    let output = Command::new("brightnessctl").arg("-m").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split(',')
        .nth(3)
        .and_then(|percent| percent.trim().trim_end_matches('%').parse::<f64>().ok())
}

fn spawn_quiet(program: &str, args: &[&str]) {
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Opens a KDE settings module (Plasma 6 first, then Plasma 5).
fn open_kcm(module: &str) {
    let module = module.to_string();
    let _ = std::thread::spawn(move || {
        let ok = Command::new("kcmshell6")
            .arg(&module)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        if !ok {
            spawn_quiet("kcmshell5", &[&module]);
        }
    });
}

// ----------------------------------------------------------------- drawing

/// The laptop-like display picture at the top of the page.
fn monitor_picture() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(130);
    area.set_content_height(84);
    area.set_halign(gtk::Align::Center);
    area.set_draw_func(|_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let rounded = |x: f64, y: f64, rw: f64, rh: f64, r: f64| {
            cr.new_sub_path();
            cr.arc(x + rw - r, y + r, r, -0.5 * PI, 0.0);
            cr.arc(x + rw - r, y + rh - r, r, 0.0, 0.5 * PI);
            cr.arc(x + r, y + rh - r, r, 0.5 * PI, PI);
            cr.arc(x + r, y + r, r, PI, 1.5 * PI);
            cr.close_path();
        };
        // Bezel.
        rounded(10.0, 2.0, w - 20.0, h - 12.0, 5.0);
        cr.set_source_rgb(0.62, 0.63, 0.66);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(0.08, 0.08, 0.09);
        let _ = cr.fill();
        rounded(11.2, 3.2, w - 22.4, h - 14.4, 4.0);
        cr.set_source_rgb(0.04, 0.04, 0.05);
        let _ = cr.fill();
        // Screen: a sunset.
        rounded(14.0, 6.0, w - 28.0, h - 20.0, 2.0);
        let sky = gtk::cairo::LinearGradient::new(0.0, 6.0, 0.0, h - 14.0);
        sky.add_color_stop_rgb(0.0, 0.21, 0.27, 0.42);
        sky.add_color_stop_rgb(0.45, 0.78, 0.42, 0.28);
        sky.add_color_stop_rgb(0.75, 0.95, 0.60, 0.30);
        sky.add_color_stop_rgb(1.0, 0.22, 0.14, 0.14);
        let _ = cr.set_source(&sky);
        let _ = cr.fill();
        // Base.
        rounded(2.0, h - 10.0, w - 4.0, 4.0, 2.0);
        let base = gtk::cairo::LinearGradient::new(0.0, h - 10.0, 0.0, h - 6.0);
        base.add_color_stop_rgb(0.0, 0.86, 0.87, 0.89);
        base.add_color_stop_rgb(1.0, 0.58, 0.59, 0.62);
        let _ = cr.set_source(&base);
        let _ = cr.fill();
    });
    area
}

fn sun_glyph(size: i32, big: bool) -> gtk::DrawingArea {
    glyph(size, size, move |cr, w, _h, ink| {
        let (c, s) = (w / 2.0, w);
        ink(0.85);
        let radius = s * if big { 0.23 } else { 0.17 };
        cr.arc(c, c, radius, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.set_line_width(if big { 1.6 } else { 1.3 });
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        for i in 0..8 {
            let a = i as f64 * PI / 4.0;
            cr.move_to(c + a.cos() * s * 0.34, c + a.sin() * s * 0.34);
            cr.line_to(c + a.cos() * s * 0.46, c + a.sin() * s * 0.46);
            let _ = cr.stroke();
        }
    })
}

// -------------------------------------------------------------------- page

#[derive(Clone)]
struct Ui {
    name: gtk::Label,
    list: gtk::Box,
    show_all: Rc<Cell<bool>>,
    output: Rc<RefCell<Option<Output>>>,
    slot: Slot,
}

/// Switches a display back to `mode_id`.
fn revert_mode(output: &str, mode_id: &str, slot: &Slot) {
    let (output, mode_id, slot) = (output.to_string(), mode_id.to_string(), Rc::clone(slot));
    glib::spawn_future_local(async move {
        let _ = gtk::gio::spawn_blocking(move || {
            let _ = Command::new("kscreen-doctor")
                .arg(format!("output.{output}.mode.{mode_id}"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        })
        .await;
        refresh_later(&slot, 600);
    });
}

const CONFIRM_SECONDS: u32 = 15;

/// "Keep these display settings?": counts down and switches back by itself,
/// so a mode that leaves the screen unusable fixes itself.
fn confirm_or_revert(parent: Option<gtk::Window>, output: String, previous: String, slot: Slot) {
    let dialog = gtk::Window::builder()
        .title("Keep Display Settings?")
        .modal(true)
        .resizable(false)
        .default_width(360)
        .css_classes(["displays-confirm"])
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let heading = label("Keep these display settings?", &["settings-row-title"]);
    let countdown = label("", &["settings-row-subtitle"]);
    countdown.set_wrap(true);
    let revert_button = crate::wifi_page::action_button("Revert", || {});
    let keep_button = crate::wifi_page::action_button("Keep", || {});
    keep_button.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(14);
    buttons.append(&revert_button);
    buttons.append(&keep_button);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.set_margin_top(18);
    content.set_margin_bottom(16);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.append(&heading);
    content.append(&countdown);
    content.append(&buttons);
    dialog.set_child(Some(&content));

    let remaining = Rc::new(Cell::new(CONFIRM_SECONDS));
    let finished = Rc::new(Cell::new(false));
    let text = move |seconds: u32| {
        format!(
            "The previous settings come back automatically in {seconds} second{} unless you keep these.",
            if seconds == 1 { "" } else { "s" }
        )
    };
    countdown.set_text(&text(CONFIRM_SECONDS));

    // Ends the dialog: `keep` decides whether the new mode stays.
    let finish: Rc<dyn Fn(bool)> = {
        let (dialog, finished) = (dialog.clone(), Rc::clone(&finished));
        Rc::new(move |keep: bool| {
            if finished.replace(true) {
                return;
            }
            if !keep {
                revert_mode(&output, &previous, &slot);
            }
            dialog.close();
        })
    };
    {
        let finish = Rc::clone(&finish);
        keep_button.connect_clicked(move |_| finish(true));
    }
    {
        let finish = Rc::clone(&finish);
        revert_button.connect_clicked(move |_| finish(false));
    }
    {
        // Closing the window any other way counts as not confirming.
        let finish = Rc::clone(&finish);
        dialog.connect_close_request(move |_| {
            finish(false);
            glib::Propagation::Proceed
        });
    }
    {
        let (finish, finished) = (Rc::clone(&finish), Rc::clone(&finished));
        glib::timeout_add_seconds_local(1, move || {
            if finished.get() {
                return glib::ControlFlow::Break;
            }
            let left = remaining.get().saturating_sub(1);
            remaining.set(left);
            countdown.set_text(&text(left));
            if left == 0 {
                finish(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
    crate::preferences::apply_dark_mode(&dialog);
    dialog.present();
}

fn resolution_row(
    mode: &Mode,
    selected: bool,
    suffix: &str,
    apply: Option<(String, String, String)>,
    ui: &Ui,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("displays-res-row");
    if selected {
        row.add_css_class("displays-res-selected");
    }
    row.append(&label(&format!("{} \u{00d7} {}{}", mode.width, mode.height, suffix), &["displays-res-text"]));
    if let (Some((output, mode_id, previous)), false) = (apply, selected) {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let slot = Rc::clone(&ui.slot);
        click.connect_released(move |gesture, _, _, _| {
            let (output, mode_id, previous, slot) =
                (output.clone(), mode_id.clone(), previous.clone(), Rc::clone(&slot));
            let parent = gesture.widget().and_then(|widget| widget.root()).and_downcast::<gtk::Window>();
            glib::spawn_future_local(async move {
                let (apply_output, apply_mode) = (output.clone(), mode_id.clone());
                let ok = gtk::gio::spawn_blocking(move || {
                    Command::new("kscreen-doctor")
                        .arg(format!("output.{apply_output}.mode.{apply_mode}"))
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .is_ok_and(|status| status.success())
                })
                .await
                .unwrap_or(false);
                refresh_later(&slot, 600);
                // Only a change that really happened needs confirming.
                if ok && previous != mode_id {
                    confirm_or_revert(parent, output, previous, slot);
                }
            });
        });
        row.add_controller(click);
    }
    row
}

fn apply(ui: &Ui, output: Option<Output>) {
    clear(&ui.list);
    *ui.output.borrow_mut() = output.clone();

    let Some(output) = output else {
        // No kscreen-doctor: show the size GTK reports, read-only.
        ui.name.set_text("Built-in Display");
        let size = gtk::gdk::Display::default()
            .and_then(|display| display.monitors().item(0))
            .and_then(|item| item.downcast::<gtk::gdk::Monitor>().ok())
            .map(|monitor| monitor.geometry());
        if let Some(geometry) = size {
            let mode = Mode { id: String::new(), width: geometry.width() as i64, height: geometry.height() as i64, refresh: 0.0 };
            ui.list.append(&resolution_row(&mode, true, "", None, ui));
        }
        return;
    };
    ui.name.set_text(&output.label);

    let all = resolutions(&output);
    let current = output.modes.iter().find(|m| m.id == output.current).cloned();
    let default = output.preferred.as_ref().and_then(|id| output.modes.iter().find(|m| &m.id == id)).cloned();

    let mut shown: Vec<Mode> = if ui.show_all.get() || all.len() <= 4 {
        all.clone()
    } else {
        // The current and default sizes, filled up to four from the top.
        let mut picked: Vec<Mode> = Vec::new();
        for extra in current.iter().chain(default.iter()).chain(all.iter()) {
            if picked.len() < 4 && !picked.iter().any(|m| same_size(m, extra)) {
                if let Some(entry) = all.iter().find(|m| same_size(m, extra)) {
                    picked.push(entry.clone());
                }
            }
        }
        picked.sort_by_key(|m| std::cmp::Reverse(m.width * m.height));
        picked
    };
    shown.dedup_by(|a, b| same_size(a, b));

    for (index, mode) in shown.iter().enumerate() {
        if index > 0 {
            ui.list.append(&separator());
        }
        let selected = current.as_ref().is_some_and(|c| same_size(c, mode));
        let suffix = if default.as_ref().is_some_and(|d| same_size(d, mode)) { " (Default)" } else { "" };
        ui.list.append(&resolution_row(mode, selected, suffix, Some((output.name.clone(), mode.id.clone(), output.current.clone())), ui));
    }
}

pub fn build(body: &gtk::Box) {
    // ---- header: picture, name, add menu ----
    let picture_column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    picture_column.set_halign(gtk::Align::Center);
    picture_column.append(&monitor_picture());
    let name = label("Built-in Display", &["displays-name"]);
    name.set_halign(gtk::Align::Center);
    picture_column.append(&name);

    let add = gtk::Button::new();
    add.add_css_class("wifi-action");
    add.add_css_class("displays-add");
    let add_contents = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_contents.append(&gtk::Label::new(Some("+")));
    add_contents.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
    add.set_child(Some(&add_contents));
    add.set_halign(gtk::Align::End);
    add.set_valign(gtk::Align::End);
    let add_menu = ContextMenu::new(gtk::PositionType::Bottom, &["settings-popup-menu"]);
    add_menu.set_parent(&add);
    add_menu.append_action("Arrange Displays\u{2026}", true, || open_kcm("kcm_kscreen"));
    {
        let menu = add_menu.clone();
        add.connect_clicked(move |button| menu.popup_at(button.width() / 2, button.height()));
    }
    add.connect_destroy(move |_| add_menu.unparent());

    let header = gtk::Overlay::new();
    header.add_css_class("displays-header");
    header.set_child(Some(&picture_column));
    header.add_overlay(&add);
    body.append(&header);

    // ---- resolutions ----
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("displays-res-list");
    let show_all = Rc::new(Cell::new(false));
    let (show_row, show_switch) = switch_row("Show all resolutions", None, false);
    show_row.add_css_class("displays-show-all");

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("settings-group");
    card.add_css_class("wifi-group");
    card.add_css_class("displays-res-card");
    card.append(&list);
    card.append(&show_row);
    body.append(&card);

    let slot: Slot = Rc::new(RefCell::new(None));
    let ui = Ui { name, list, show_all: Rc::clone(&show_all), output: Rc::new(RefCell::new(None)), slot: Rc::clone(&slot) };
    let refresh: Rc<dyn Fn()> = {
        let ui = ui.clone();
        Rc::new(move || {
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                if let Ok(output) = gtk::gio::spawn_blocking(read_output).await {
                    apply(&ui, output);
                }
            });
        })
    };
    *slot.borrow_mut() = Some(refresh);
    {
        let (show_all, slot) = (Rc::clone(&show_all), Rc::clone(&slot));
        show_switch.connect_active_notify(move |switch| {
            show_all.set(switch.is_active());
            refresh_now(&slot);
        });
    }

    // ---- brightness, auto brightness, True Tone ----
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_width_request(250);
    scale.set_value(100.0);
    let slider = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    slider.append(&sun_glyph(16, false));
    slider.append(&scale);
    slider.append(&sun_glyph(20, true));

    let brightness = settings_row("Brightness", None, &slider);
    let (auto_row, auto_switch) = switch_row(
        "Automatically adjust brightness",
        Some("\u{26a0}\u{fe0f} Energy usage may be higher when Automatically Adjust Brightness is off. Display quality and battery performance may also be reduced, and maximum brightness levels could be limited."),
        false,
    );
    auto_switch.set_valign(gtk::Align::Start);
    let (tone_row, tone_switch) = switch_row(
        "True Tone",
        Some("Automatically adapt display to make colours appear consistent in different ambient lighting conditions."),
        false,
    );
    tone_switch.set_valign(gtk::Align::Start);
    body.append(&settings_group(&[brightness, auto_row, tone_row]));

    {
        // Read the real brightness; ignore the change this causes.
        let updating = Rc::new(Cell::new(true));
        let pending: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
        let timer_running = Rc::new(Cell::new(false));
        {
            let (scale, updating) = (scale.clone(), Rc::clone(&updating));
            glib::spawn_future_local(async move {
                match gtk::gio::spawn_blocking(brightness_percent).await {
                    Ok(Some(percent)) => scale.set_value(percent.max(1.0)),
                    _ => scale.set_sensitive(false),
                }
                updating.set(false);
            });
        }
        scale.connect_value_changed(move |scale| {
            if updating.get() {
                return;
            }
            pending.set(Some(scale.value()));
            if timer_running.replace(true) {
                return;
            }
            let (pending, timer_running) = (Rc::clone(&pending), Rc::clone(&timer_running));
            glib::timeout_add_local_once(Duration::from_millis(80), move || {
                timer_running.set(false);
                if let Some(value) = pending.take() {
                    spawn_quiet("brightnessctl", &["set", &format!("{}%", value.round() as i32)]);
                }
            });
        });
    }

    // ---- colour profile ----
    body.append(&settings_group(&[popup_row(
        "Colour profile",
        &["Generic RGB Profile", "sRGB IEC61966-2.1", "Display P3"],
        0,
    )]));

    // ---- TV ----
    let tv = settings_row(
        "When connected to TV",
        Some("Choose what to show or use the TV as a secondary display."),
        &popup_button(&["Ask What to Show", "Mirror Displays", "Use as Separate Display"], 0),
    );
    body.append(&settings_group(&[tv]));

    // ---- buttons ----
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let advanced = crate::wifi_page::action_button("Advanced\u{2026}", || open_kcm("kcm_kscreen"));
    let night = crate::wifi_page::action_button("Night Shift\u{2026}", || open_kcm("kcm_nightlight"));
    let help = gtk::Button::with_label("?");
    help.add_css_class("bt-help");
    help.connect_clicked(|_| {
        let _ = gtk::gio::AppInfo::launch_default_for_uri("https://userbase.kde.org/Tutorials/Display", None::<&gtk::gio::AppLaunchContext>);
    });
    buttons.append(&advanced);
    buttons.append(&night);
    buttons.append(&help);
    body.append(&buttons);

    body.connect_map({
        let slot = Rc::clone(&slot);
        move |_| refresh_now(&slot)
    });
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
    body.connect_destroy(move |_| {
        slot.borrow_mut().take();
    });
}
