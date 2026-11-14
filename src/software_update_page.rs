//! General > Software Update: checks for and installs updates with DNF, and
//! shows the Fedora release plus the KWin and kernel versions it is running.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::settings::{settings_group, settings_row};
use crate::wifi_page::{action_button, glyph, label, separator};

const SECURITY_URL: &str = "https://bodhi.fedoraproject.org/";
const DNF_DOCS_URL: &str = "https://docs.fedoraproject.org/en-US/quick-docs/dnf/";
const LICENSE_URL: &str = "https://docs.fedoraproject.org/en-US/legal/license-approval/";

// ------------------------------------------------------------------- data

#[derive(Clone)]
enum Status {
    Checking,
    Installing,
    UpToDate,
    /// Package names with their new versions.
    Available(Vec<(String, String)>),
    Installed,
    Failed(String),
}

enum Outcome {
    Updates(Vec<(String, String)>),
    UpToDate,
    Failed(String),
}

fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// "Fedora Linux 44" from /etc/os-release.
fn release_name() -> String {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .map(|value| value.trim().trim_matches('"').to_string())
    };
    match (field("NAME="), field("VERSION_ID=")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        _ => "Fedora Linux".to_string(),
    }
}

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// "6.4.5" from `kwin_wayland --version` ("kwin 6.4.5"), or the kwin package.
fn kwin_version() -> Option<String> {
    for program in ["kwin_wayland", "kwin_x11"] {
        if let Some(text) = command_text(program, &["--version"]) {
            if let Some(version) = text.split_whitespace().last() {
                return Some(version.to_string());
            }
        }
    }
    command_text("rpm", &["-q", "--qf", "%{VERSION}", "kwin"]).filter(|text| !text.contains(' '))
}

fn automatic_updates_on() -> bool {
    ["dnf5-automatic.timer", "dnf-automatic.timer", "dnf-automatic-install.timer", "dnf5-automatic-install.timer"]
        .iter()
        .any(|unit| command_text("systemctl", &["is-enabled", unit]).as_deref() == Some("enabled"))
}

/// Asks DNF what could be upgraded. Blocking and possibly slow (it refreshes
/// the repository metadata), so run it off the main thread.
fn check_updates() -> Outcome {
    let output = match Command::new("dnf")
        .args(["check-upgrade", "--refresh", "-q"])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(_) => return Outcome::Failed("DNF is not installed.".into()),
    };
    match output.status.code() {
        Some(0) => Outcome::UpToDate,
        Some(100) => {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut packages = Vec::new();
            for line in text.lines() {
                if line.starts_with("Obsoleting") {
                    break;
                }
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() == 3 && fields[0].contains('.') && fields[1].chars().any(|c| c.is_ascii_digit()) {
                    let name = fields[0].rsplit_once('.').map_or(fields[0], |(name, _arch)| name);
                    packages.push((name.to_string(), fields[1].to_string()));
                }
            }
            if packages.is_empty() {
                Outcome::UpToDate
            } else {
                Outcome::Updates(packages)
            }
        }
        _ => {
            let error = String::from_utf8_lossy(&output.stderr);
            let first = error.lines().find(|line| !line.trim().is_empty()).unwrap_or("DNF could not check for updates.");
            Outcome::Failed(first.trim().to_string())
        }
    }
}

/// Installs everything with DNF, asking for the password through polkit.
fn install_updates() -> Result<(), String> {
    let status = Command::new("pkexec")
        .args(["dnf", "upgrade", "-y", "--refresh"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "pkexec is not available.".to_string())?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) | Some(127) => Err("Authentication was cancelled.".into()),
        _ => Err("DNF could not install the updates.".into()),
    }
}

// ----------------------------------------------------------------- drawing

fn update_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(40);
    area.set_content_height(40);
    area.set_valign(gtk::Align::Start);
    area.set_draw_func(|_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let (cx, cy, r) = (w / 2.0, h / 2.0, w / 2.0 - 1.0);
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let fill = gtk::cairo::LinearGradient::new(0.0, 0.0, w, h);
        fill.add_color_stop_rgb(0.0, 0.42, 0.78, 1.0);
        fill.add_color_stop_rgb(0.55, 0.12, 0.42, 0.92);
        fill.add_color_stop_rgb(1.0, 0.07, 0.17, 0.62);
        let _ = cr.set_source(&fill);
        let _ = cr.fill_preserve();
        cr.clip();
        // Soft waves.
        for (offset, alpha) in [(0.0, 0.30), (7.0, 0.20), (14.0, 0.12)] {
            cr.set_source_rgba(1.0, 1.0, 1.0, alpha);
            cr.move_to(0.0, h * 0.55 + offset);
            cr.curve_to(w * 0.3, h * 0.25 + offset, w * 0.6, h * 0.95 + offset, w, h * 0.45 + offset);
            cr.line_to(w, h);
            cr.line_to(0.0, h);
            cr.close_path();
            let _ = cr.fill();
        }
    });
    area
}

/// The circled "i" button beside a row.
fn info_button(tooltip: &str, on_click: impl Fn() + 'static) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("wifi-more");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(tooltip));
    let glyph = glyph(20, 20, |cr, w, h, ink| {
        let (cx, cy) = (w / 2.0, h / 2.0);
        ink(0.6);
        cr.set_line_width(1.3);
        cr.arc(cx, cy, 8.6, 0.0, 2.0 * PI);
        let _ = cr.stroke();
        ink(0.75);
        cr.arc(cx, cy - 3.6, 1.1, 0.0, 2.0 * PI);
        let _ = cr.fill();
        cr.set_line_width(1.7);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.move_to(cx, cy - 0.6);
        cr.line_to(cx, cy + 4.6);
        let _ = cr.stroke();
    });
    button.set_child(Some(&glyph));
    button.connect_clicked(move |_| on_click());
    button
}

fn open_uri(uri: &str) {
    let _ = gtk::gio::AppInfo::launch_default_for_uri(uri, None::<&gtk::gio::AppLaunchContext>);
}

fn paragraph(markup: &str) -> gtk::Label {
    let text = gtk::Label::new(None);
    text.set_markup(markup);
    text.set_halign(gtk::Align::Start);
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.add_css_class("settings-row-subtitle");
    text.add_css_class("su-text");
    text
}

// -------------------------------------------------------------------- page

#[derive(Clone)]
struct Ui {
    title: gtk::Label,
    subtitle: gtk::Label,
    description: gtk::Label,
    button: gtk::Button,
    spinner: gtk::Spinner,
    status: Rc<RefCell<Status>>,
    last_check: Rc<Cell<Option<Instant>>>,
    kernel: String,
    kwin: Rc<RefCell<Option<String>>>,
}

fn security_paragraph() -> String {
    format!("For information on the security content of Fedora updates, please visit:\n<a href=\"{SECURITY_URL}\">{SECURITY_URL}</a>")
}

fn show(ui: &Ui, status: Status) {
    *ui.status.borrow_mut() = status.clone();
    let mut linux = String::new();
    if let Some(kwin) = ui.kwin.borrow().as_ref() {
        linux.push_str(&format!("KWin {kwin}, "));
    }
    linux.push_str(&format!("Linux {}", ui.kernel.split('-').next().unwrap_or(&ui.kernel)));
    let busy = matches!(status, Status::Checking | Status::Installing);
    ui.spinner.set_spinning(busy);
    ui.spinner.set_visible(busy);
    ui.button.set_visible(!busy);
    let (summary, text, button) = match &status {
        Status::Checking => ("Checking for updates\u{2026}".to_string(), "Checking for updates with DNF\u{2026}".to_string(), ""),
        Status::Installing => (
            "Installing updates\u{2026}".to_string(),
            "Installing with DNF. You may be asked for your password.".to_string(),
            "",
        ),
        Status::UpToDate => ("Up to date".to_string(), "Your system is up to date.".to_string(), "Check for Updates"),
        Status::Installed => (
            "Updates installed".to_string(),
            "The updates were installed. Restart your computer to finish applying them.".to_string(),
            "Check for Updates",
        ),
        Status::Failed(error) => (
            "Couldn\u{2019}t check for updates".to_string(),
            glib::markup_escape_text(error).to_string(),
            "Try Again",
        ),
        Status::Available(packages) => {
            let count = packages.len();
            let names: Vec<&str> = packages.iter().take(3).map(|(name, _)| name.as_str()).collect();
            let more = count.saturating_sub(names.len());
            let mut text = format!("{} included: {}", if count == 1 { "1 update is" } else { "Updates are" }, names.join(", "));
            if more > 0 {
                text.push_str(&format!(" and {more} more"));
            }
            text.push('.');
            (
                format!("{count} update{} available", if count == 1 { "" } else { "s" }),
                glib::markup_escape_text(&text).to_string(),
                "Update Now",
            )
        }
    };
    ui.subtitle.set_text(&format!("{linux} \u{2014} {summary}"));
    ui.description.set_markup(&format!("{text}\n\n{}", security_paragraph()));
    if !button.is_empty() {
        ui.button.set_label(button);
    }
}

fn start_check(ui: &Ui) {
    if matches!(*ui.status.borrow(), Status::Checking | Status::Installing) {
        return;
    }
    show(ui, Status::Checking);
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let outcome = gtk::gio::spawn_blocking(check_updates).await;
        ui.last_check.set(Some(Instant::now()));
        show(
            &ui,
            match outcome {
                Ok(Outcome::Updates(packages)) => Status::Available(packages),
                Ok(Outcome::UpToDate) => Status::UpToDate,
                Ok(Outcome::Failed(error)) => Status::Failed(error),
                Err(_) => Status::Failed("The check was interrupted.".into()),
            },
        );
    });
}

fn start_install(ui: &Ui) {
    show(ui, Status::Installing);
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = gtk::gio::spawn_blocking(install_updates).await;
        match result {
            Ok(Ok(())) => show(&ui, Status::Installed),
            Ok(Err(error)) => show(&ui, Status::Failed(error)),
            Err(_) => show(&ui, Status::Failed("The installation was interrupted.".into())),
        }
    });
}

pub fn build(body: &gtk::Box) {
    // ---- top card ----
    let title = label(&release_name(), &["settings-row-title", "su-title"]);
    let subtitle = label("", &["settings-row-subtitle"]);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&title);
    text.append(&subtitle);

    let spinner = gtk::Spinner::new();
    spinner.set_valign(gtk::Align::Center);
    spinner.set_margin_end(6);
    let button = action_button("Check for Updates", || {});
    let info = info_button("Show the packages being updated", || {});
    let packages_popover = gtk::Popover::new();
    packages_popover.set_parent(&info);
    packages_popover.set_position(gtk::PositionType::Bottom);
    let packages_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    packages_list.set_margin_top(10);
    packages_list.set_margin_bottom(10);
    packages_list.set_margin_start(12);
    packages_list.set_margin_end(12);
    let packages_scroll = gtk::ScrolledWindow::new();
    packages_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    packages_scroll.set_propagate_natural_width(true);
    packages_scroll.set_propagate_natural_height(true);
    packages_scroll.set_max_content_height(320);
    packages_scroll.set_child(Some(&packages_list));
    packages_popover.set_child(Some(&packages_scroll));
    {
        let popover = packages_popover.clone();
        info.connect_destroy(move |_| popover.unparent());
    }

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.add_css_class("settings-row");
    top.add_css_class("su-top-row");
    top.append(&update_icon());
    top.append(&text);
    top.append(&spinner);
    top.append(&button);
    top.append(&info);

    let description = paragraph("");
    let description_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    description_box.add_css_class("settings-row");
    description_box.append(&description);

    let footer = paragraph(&format!(
        "Updates are downloaded and installed with DNF. <a href=\"{DNF_DOCS_URL}\">More Info\u{2026}</a>"
    ));
    let footer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer_box.add_css_class("settings-row");
    footer_box.append(&footer);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("settings-group");
    card.add_css_class("wifi-group");
    card.append(&top);
    card.append(&separator());
    card.append(&description_box);
    card.append(&separator());
    card.append(&footer_box);
    body.append(&card);

    // ---- kernel / automatic updates ----
    let kernel = kernel_release();
    let kernel_value = label(&kernel, &["su-value"]);
    let kernel_row = settings_row("Kernel", None, &kernel_value);
    body.append(&settings_group(&[kernel_row]));

    let automatic_value = label("\u{2026}", &["su-value"]);
    let automatic_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    automatic_box.append(&automatic_value);
    automatic_box.append(&info_button(
        "Automatic updates use the dnf-automatic timer.",
        || open_uri("https://dnf5.readthedocs.io/en/latest/dnf5.8.html"),
    ));
    body.append(&settings_group(&[settings_row("Automatic Updates", None, &automatic_box)]));

    // ---- licence ----
    let licence = paragraph(&format!(
        "Use of this software is subject to the licence agreements of the individual packages being updated. <a href=\"{LICENSE_URL}\">Learn more\u{2026}</a>"
    ));
    let licence_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    licence_box.add_css_class("settings-row");
    licence_box.append(&licence);
    body.append(&settings_group(&[licence_box]));

    let help = gtk::Button::with_label("?");
    help.add_css_class("bt-help");
    help.set_halign(gtk::Align::End);
    help.set_margin_top(14);
    help.connect_clicked(|_| open_uri(DNF_DOCS_URL));
    body.append(&help);

    // ---- behaviour ----
    let ui = Ui {
        title,
        subtitle,
        description,
        button: button.clone(),
        spinner,
        status: Rc::new(RefCell::new(Status::UpToDate)),
        last_check: Rc::new(Cell::new(None)),
        kernel,
        kwin: Rc::new(RefCell::new(None)),
    };
    show(&ui, Status::UpToDate);

    {
        let (ui, popover, list) = (ui.clone(), packages_popover.clone(), packages_list.clone());
        info.connect_clicked(move |_| {
            crate::wifi_page::clear(&list);
            match &*ui.status.borrow() {
                Status::Available(packages) => {
                    list.append(&label(
                        &format!("{} package{} to update", packages.len(), if packages.len() == 1 { "" } else { "s" }),
                        &["settings-row-title"],
                    ));
                    for (name, version) in packages {
                        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
                        let name_label = label(name, &["su-text"]);
                        name_label.set_hexpand(true);
                        let version_label = label(version, &["settings-row-subtitle"]);
                        version_label.set_halign(gtk::Align::End);
                        row.append(&name_label);
                        row.append(&version_label);
                        list.append(&row);
                    }
                }
                Status::Checking => list.append(&label("Checking for updates\u{2026}", &["su-text"])),
                Status::Installing => list.append(&label("Installing updates\u{2026}", &["su-text"])),
                Status::Failed(_) => list.append(&label("The update check failed.", &["su-text"])),
                Status::UpToDate | Status::Installed => list.append(&label("No packages need updating.", &["su-text"])),
            }
            popover.popup();
        });
    }

    {
        let ui = ui.clone();
        button.connect_clicked(move |_| {
            let available = matches!(*ui.status.borrow(), Status::Available(_));
            if available {
                start_install(&ui);
            } else {
                start_check(&ui);
            }
        });
    }

    // KWin version and the automatic-updates state (both run commands).
    {
        let (ui, automatic_value) = (ui.clone(), automatic_value.clone());
        glib::spawn_future_local(async move {
            let version = gtk::gio::spawn_blocking(kwin_version).await.ok().flatten();
            *ui.kwin.borrow_mut() = version;
            let current = ui.status.borrow().clone();
            show(&ui, current);
            let automatic = gtk::gio::spawn_blocking(automatic_updates_on).await.unwrap_or(false);
            automatic_value.set_text(if automatic { "On" } else { "Off" });
        });
    }

    // Check when the page is opened (not more than every ten minutes).
    {
        let ui = ui.clone();
        body.connect_map(move |_| {
            let stale = ui.last_check.get().is_none_or_stale();
            if stale {
                start_check(&ui);
            }
        });
    }
}

trait StaleCheck {
    fn is_none_or_stale(&self) -> bool;
}

impl StaleCheck for Option<Instant> {
    fn is_none_or_stale(&self) -> bool {
        self.map_or(true, |at| at.elapsed() > Duration::from_secs(600))
    }
}
