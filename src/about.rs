//! "About This System": the window behind the Apple menu's first item,
//! modelled on macOS Sonoma's "About This Mac".
//!
//! Like the original it is a small, fixed-size window with a hero picture of
//! the machine, the model name, a short spec table and a "More Info…" button.
//! Everything is read live from the running system (DMI, /proc, os-release,
//! lspci). A row whose value cannot be found is left out, never faked.

use std::process::Command;

use gtk::prelude::*;
use gtk::{gdk, glib};

const WINDOW_TITLE: &str = "About This System";

/// Everything the window displays.
struct SystemInfo {
    model: String,
    maker: String,
    rows: Vec<(&'static str, String)>,
    footer: String,
}

fn read_trimmed(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A DMI field, or `None` when it is unreadable or a vendor placeholder such as
/// "To Be Filled By O.E.M.".
fn dmi(field: &str) -> Option<String> {
    const PLACEHOLDERS: &[&str] = &[
        "to be filled by o.e.m.",
        "default string",
        "system product name",
        "system manufacturer",
        "system serial number",
        "not specified",
        "not applicable",
        "none",
        "unknown",
        "standard pc",
    ];
    let value = read_trimmed(&format!("/sys/class/dmi/id/{field}"))?;
    let lower = value.to_ascii_lowercase();
    if PLACEHOLDERS.iter().any(|junk| lower.starts_with(junk)) {
        None
    } else {
        Some(value)
    }
}

fn os_release(key: &str) -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name == key).then(|| value.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

/// "AMD Ryzen 7 7700X 8-Core Processor" -> "AMD Ryzen 7 7700X 8-Core".
fn processor() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let line = text.lines().find(|line| line.starts_with("model name"))?;
    let raw = line.split_once(':')?.1;
    let mut name = raw
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("(tm)", "");
    if let Some(at) = name.find(" @ ") {
        name.truncate(at);
    }
    let name = name.replace(" Processor", "").replace(" CPU", "");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some(name)
}

/// Installed memory, rounded up from what the kernel reports as usable
/// (e.g. 27.0 GiB usable -> "32 GB"), the way a spec sheet would say it.
fn memory() -> Option<String> {
    const SIZES: &[u32] = &[
        2, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024,
    ];
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: f64 = text
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let gib = kib / 1024.0 / 1024.0;
    let installed = SIZES
        .iter()
        .copied()
        .find(|size| f64::from(*size) >= gib)
        .unwrap_or(gib.ceil() as u32);
    Some(format!("{installed} GB"))
}

fn short_vendor(vendor: &str) -> String {
    if vendor.contains("AMD") || vendor.contains("Advanced Micro") {
        "AMD".to_string()
    } else if vendor.contains("NVIDIA") {
        "NVIDIA".to_string()
    } else if vendor.contains("Intel") {
        "Intel".to_string()
    } else {
        vendor.split('[').next().unwrap_or(vendor).trim().to_string()
    }
}

/// "Navi 23 [Radeon RX 6600/6600 XT/6600M]" -> "Radeon RX 6600".
fn short_device(device: &str) -> String {
    if let (Some(start), Some(end)) = (device.find('['), device.rfind(']')) {
        if start < end {
            let inner = &device[start + 1..end];
            return inner.split('/').next().unwrap_or(inner).trim().to_string();
        }
    }
    device.trim().to_string()
}

/// Graphics adapters from `lspci`, one per line. Omitted if `lspci` is absent.
fn graphics() -> Option<String> {
    let output = Command::new("lspci").arg("-mm").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        // 00:02.0 "VGA compatible controller" "Vendor" "Device" -r00 ...
        let fields: Vec<&str> = line.split('"').collect();
        if fields.len() < 6 {
            continue;
        }
        let class = fields[1];
        if !(class.contains("VGA")
            || class.contains("3D controller")
            || class.contains("Display controller"))
        {
            continue;
        }
        let vendor = short_vendor(fields[3]);
        let device = short_device(fields[5]);
        let name = if device.starts_with(&vendor) {
            device
        } else {
            format!("{vendor} {device}")
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| names.join("\n"))
}

/// The volume mounted at `/`: its filesystem label if it has one (the
/// "Macintosh HD" of Linux), otherwise the device name.
fn startup_disk() -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    let source = mounts
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let source = parts.next()?;
            let target = parts.next()?;
            (target == "/").then(|| source.to_string())
        })
        .last()?;
    if let (Ok(entries), Ok(source_real)) = (
        std::fs::read_dir("/dev/disk/by-label"),
        std::fs::canonicalize(&source),
    ) {
        for entry in entries.flatten() {
            if std::fs::canonicalize(entry.path()).is_ok_and(|real| real == source_real) {
                let label = entry.file_name().to_string_lossy().replace("\\x20", " ");
                return Some(label);
            }
        }
    }
    Some(source.rsplit('/').next().unwrap_or(&source).to_string())
}

fn system_info() -> SystemInfo {
    let os_name = os_release("NAME").unwrap_or_else(|| "Linux".to_string());
    let os = match os_release("VERSION_ID") {
        Some(version) => format!("{os_name} {version}"),
        None => os_release("PRETTY_NAME").unwrap_or(os_name),
    };

    let mut rows: Vec<(&'static str, String)> = Vec::new();
    let mut push = |label: &'static str, value: Option<String>| {
        if let Some(value) = value {
            rows.push((label, value));
        }
    };
    push("Processor", processor());
    push("Memory", memory());
    push("Graphics", graphics());
    push("Startup Disk", startup_disk());
    // Readable only as root on most systems; shown when it is available.
    push("Serial Number", dmi("product_serial"));
    push("OS", Some(os));

    let kernel = read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_default();
    SystemInfo {
        model: dmi("product_name")
            .or_else(|| dmi("board_name"))
            .unwrap_or_else(|| "Computer".to_string()),
        maker: dmi("sys_vendor")
            .or_else(|| dmi("board_vendor"))
            .unwrap_or_else(|| "Linux".to_string()),
        rows,
        footer: format!(
            "Linux {kernel}\nSonoma Desktop {}",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

/// A small desktop display showing the bundled wallpaper: the stand-in for
/// the Mac photograph at the top of "About This Mac".
fn hero() -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_halign(gtk::Align::Center);
    column.add_css_class("about-hero");

    let bezel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bezel.add_css_class("about-monitor");
    bezel.set_halign(gtk::Align::Center);
    // Clip the picture to the bezel's rounded corners.
    bezel.set_overflow(gtk::Overflow::Hidden);
    // A pre-cropped 176x108 thumbnail of the wallpaper. Using the full
    // wallpaper here made the window as big as the picture's natural size.
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!(
        "../assets/about-hero.jpg"
    ))) {
        Ok(texture) => {
            // The thumbnail is already 176x108, so its natural size is the
            // size we want and needs no wrapper.
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_content_fit(gtk::ContentFit::Fill);
            picture.set_size_request(176, 108);
            picture.set_can_target(false);
            bezel.append(&picture);
        }
        Err(_) => bezel.set_size_request(176, 108),
    }

    let neck = gtk::Box::new(gtk::Orientation::Vertical, 0);
    neck.add_css_class("about-monitor-neck");
    neck.set_size_request(26, 14);
    neck.set_halign(gtk::Align::Center);

    let foot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    foot.add_css_class("about-monitor-foot");
    foot.set_size_request(84, 6);
    foot.set_halign(gtk::Align::Center);

    column.append(&bezel);
    column.append(&neck);
    column.append(&foot);
    column
}

/// Close button plus two inert dots: the window cannot be resized or
/// minimised, exactly like the macOS original.
fn traffic_lights(window: &gtk::ApplicationWindow) -> gtk::Box {
    let lights = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lights.add_css_class("settings-window-controls");
    lights.set_halign(gtk::Align::Start);
    lights.set_valign(gtk::Align::Center);

    let close = gtk::Button::new();
    close.add_css_class("settings-window-control");
    close.add_css_class("close");
    close.set_size_request(14, 14);
    let window = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = window.upgrade() {
            window.close();
        }
    });
    lights.append(&close);

    for _ in 0..2 {
        let dot = gtk::Button::new();
        dot.add_css_class("settings-window-control");
        dot.add_css_class("about-inert");
        dot.set_size_request(14, 14);
        dot.set_focusable(false);
        dot.set_can_target(false);
        lights.append(&dot);
    }
    lights
}

/// Presents the About window, creating it on first use.
pub fn present(app: &gtk::Application) {
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|window| window.title().as_deref() == Some(WINDOW_TITLE))
    {
        window.present();
        return;
    }

    let info = system_info();

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .resizable(false)
        .css_classes(["sonoma-settings-window", "sonoma-about-window"])
        .build();
    // Same trick as System Settings: an empty title bar makes GTK draw the
    // frame, so the traffic lights can sit inside the content.
    let titlebar_stub = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    titlebar_stub.add_css_class("settings-titlebar-stub");
    window.set_titlebar(Some(&titlebar_stub));

    let top = gtk::WindowHandle::new();
    top.add_css_class("about-top");
    top.set_child(Some(&traffic_lights(&window)));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("about-content");
    content.set_size_request(300, -1);
    content.append(&hero());

    let model = gtk::Label::new(Some(&info.model));
    model.add_css_class("about-model");
    model.set_ellipsize(gtk::pango::EllipsizeMode::End);
    model.set_max_width_chars(24);
    content.append(&model);

    let maker = gtk::Label::new(Some(&info.maker));
    maker.add_css_class("about-maker");
    maker.set_ellipsize(gtk::pango::EllipsizeMode::End);
    maker.set_max_width_chars(32);
    content.append(&maker);

    let grid = gtk::Grid::new();
    grid.add_css_class("about-info");
    grid.set_column_spacing(10);
    grid.set_row_spacing(5);
    grid.set_halign(gtk::Align::Center);
    for (index, (label, value)) in info.rows.iter().enumerate() {
        let key = gtk::Label::new(Some(label));
        key.add_css_class("about-info-label");
        key.set_xalign(1.0);
        key.set_halign(gtk::Align::End);
        key.set_valign(gtk::Align::Start);

        let value = gtk::Label::new(Some(value));
        value.add_css_class("about-info-value");
        value.set_xalign(0.0);
        value.set_halign(gtk::Align::Start);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value.set_max_width_chars(24);

        grid.attach(&key, 0, index as i32, 1, 1);
        grid.attach(&value, 1, index as i32, 1, 1);
    }
    content.append(&grid);

    let more_info = gtk::Button::with_label("More Info…");
    more_info.add_css_class("about-more-info");
    more_info.set_halign(gtk::Align::Center);
    let settings_app = app.clone();
    more_info.connect_clicked(move |_| crate::settings::present(&settings_app));
    content.append(&more_info);

    let footer = gtk::Label::new(Some(&info.footer));
    footer.add_css_class("about-footer");
    footer.set_justify(gtk::Justification::Center);
    content.append(&footer);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("settings-root");
    root.set_overflow(gtk::Overflow::Hidden);
    root.append(&top);
    root.append(&content);
    window.set_child(Some(&root));

    crate::preferences::apply_dark_mode(&window);
    crate::context_menu::attach_window_blur(&window);
    window.present();
}
