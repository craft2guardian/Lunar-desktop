//! Developer-only preview of the Liquid Glass surfaces, rendered off-compositor.
//!
//! `LUNAR_GLASS_PREVIEW=<scene>` starts this instead of the desktop shell
//! (scenes: `cc`, `menu`, `dock`, `all`). It builds the real Control Centre
//! widgets and stand-ins for the menu and Dock shelf, lays them over a
//! wallpaper, supplies a blurred copy of that wallpaper where KWin would
//! supply the backdrop blur, and writes a 2x PNG to `LUNAR_GLASS_OUT`.
//!
//! It exists so the glass can be judged and tuned against reference
//! screenshots on a machine that is not running KWin.

use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, graphene};

use crate::liquid_glass::{GlassSurface, Material, Radius};

const SCALE: f32 = 2.0;

pub fn run(app: &gtk::Application, scene: &str) {
    let display = gdk::Display::default().expect("no display");
    let _css = crate::style::install(&display);
    if std::env::var_os("LUNAR_GLASS_DARK").is_some() {
        crate::preferences::set_dark_mode(true, app);
    }

    let wallpaper_path = std::env::var("LUNAR_GLASS_WALLPAPER").unwrap_or_else(|_| "assets/wallpaper.jpg".into());
    let texture = gdk::Texture::from_filename(&wallpaper_path).expect("wallpaper");
    let (win_w, win_h) = (1000.0_f32, 620.0_f32);

    // Stand-in for KWin's blur: the wallpaper (cover-fitted), blurred.
    {
        let texture = texture.clone();
        crate::liquid_glass::set_preview_backdrop(Some(Rc::new(move |snapshot: &gtk::Snapshot, origin: &graphene::Rect, blur_radius: f32| {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(-origin.x(), -origin.y()));
            snapshot.push_blur(blur_radius as f64);
            snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, win_w, win_h));
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            let k = (win_w / tw).max(win_h / th);
            snapshot.append_texture(&texture, &graphene::Rect::new((win_w - tw * k) / 2.0, (win_h - th * k) / 2.0, tw * k, th * k));
            snapshot.pop();
            snapshot.pop();
            snapshot.restore();
        })));
    }

    let overlay = gtk::Overlay::new();
    let picture = gtk::Picture::for_paintable(&texture);
    picture.set_content_fit(gtk::ContentFit::Cover);
    overlay.set_child(Some(&picture));

    let fixed = gtk::Fixed::new();
    overlay.add_overlay(&fixed);

    let hover_demo = std::env::var_os("LUNAR_GLASS_HOVER").is_some();
    let mut surfaces: Vec<GlassSurface> = Vec::new();

    // ---- menu bar stand-in (clock + Control Centre capsule) ------------------
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.set_size_request(win_w as i32, 28);
    bar.add_css_class("lunar-menubar");
    bar.add_css_class("bar-light-text");
    let strip = crate::glass::GlassStrip::new();
    strip.set_size_request(win_w as i32, 28);
    let bar_overlay = gtk::Overlay::new();
    bar_overlay.set_child(Some(&strip));
    let title = gtk::Label::new(Some("Finder"));
    title.add_css_class("lunar-menubar-item");
    title.set_halign(gtk::Align::Start);
    title.set_margin_start(46);
    title.set_label("Finder");
    bar_overlay.add_overlay(&title);
    let clock = gtk::Label::new(Some("Mon 7 Jul  01:52"));
    clock.add_css_class("lunar-menubar-item");
    clock.set_halign(gtk::Align::End);
    clock.set_margin_end(14);
    bar_overlay.add_overlay(&clock);
    // "File" with its menu open: the bar's open-menu capsule.
    let file = gtk::MenuButton::new();
    file.set_label("File");
    file.add_css_class("lunar-menubar-item-button");
    file.add_css_class("lunar-menubar-item");
    file.set_halign(gtk::Align::Start);
    file.set_valign(gtk::Align::Center);
    file.set_margin_start(100);
    let stub = gtk::Popover::new();
    stub.set_child(Some(&gtk::Label::new(Some(""))));
    file.set_popover(Some(&stub));
    bar_overlay.add_overlay(&file);
    {
        let file = file.clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || file.popup());
    }
    bar.append(&bar_overlay);
    fixed.put(&bar, 0.0, 0.0);

    if matches!(scene, "cc" | "all") {
        // The real Control Centre popover body.
        let button = crate::control_center::control_center_button(app);
        let popover = button.popover().expect("popover");
        let panel = popover.child().expect("panel");
        popover.set_child(None::<&gtk::Widget>);
        panel.add_css_class("cc-preview");
        if std::env::var_os("LUNAR_GLASS_DARK").is_some() {
            panel.add_css_class("dark-mode");
        }
        fixed.put(&panel, (win_w - 286.0 - 40.0) as f64, 36.0);
        set_scales(&panel, &mut [1.0, 0.82]);
        collect(&panel, &mut surfaces);
    }

    if matches!(scene, "menu" | "all") {
        let menu = file_menu();
        let wrapped = GlassSurface::wrap(&menu, Material::menu());
        fixed.put(&wrapped, 60.0, 34.0);
    }

    if matches!(scene, "dock" | "all") {
        let dock = dock_shelf();
        fixed.put(&dock, 250.0, (win_h - 120.0) as f64);
    }

    if hover_demo {
        // Hover + glint on the last bottom-row circle, like the reference.
        let circles: Vec<_> = surfaces
            .iter()
            .filter(|s| s.child().is_some_and(|c| c.has_css_class("cc-circle")))
            .collect();
        if let Some(target) = circles.last() {
            target.set_look_for_preview(1.0, 0.0, 0.3, 0.25);
        }
    }

    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_default_size(win_w as i32, win_h as i32);
    window.set_child(Some(&overlay));
    window.present();

    let out = std::env::var("LUNAR_GLASS_OUT").unwrap_or_else(|_| "glass-preview.png".into());
    let app = app.clone();
    glib::timeout_add_local_once(Duration::from_millis(1400), move || {
        capture(&window, &out, win_w, win_h);
        app.quit();
    });
}

fn set_scales(widget: &gtk::Widget, values: &mut [f64]) {
    fn walk(widget: &gtk::Widget, values: &mut std::slice::IterMut<f64>) {
        if let Some(scale) = widget.downcast_ref::<gtk::Scale>() {
            if let Some(v) = values.next() {
                scale.set_value(*v);
            }
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            walk(&c, values);
            child = c.next_sibling();
        }
    }
    walk(widget, &mut values.iter_mut());
}

fn collect(widget: &gtk::Widget, out: &mut Vec<GlassSurface>) {
    if let Some(surface) = widget.downcast_ref::<GlassSurface>() {
        out.push(surface.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        collect(&c, out);
        child = c.next_sibling();
    }
}

fn capture(window: &gtk::ApplicationWindow, path: &str, w: f32, h: f32) {
    let paintable = gtk::WidgetPaintable::new(window.child().as_ref());
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(SCALE, SCALE);
    paintable.snapshot(snapshot.upcast_ref::<gdk::Snapshot>(), w as f64, h as f64);
    let Some(node) = snapshot.to_node() else {
        eprintln!("glass-preview: nothing to render");
        return;
    };
    let renderer = window.native().and_then(|n| n.renderer()).expect("renderer");
    let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w * SCALE, h * SCALE)));
    match texture.save_to_png(path) {
        Ok(()) => eprintln!("glass-preview: wrote {path}"),
        Err(e) => eprintln!("glass-preview: {e}"),
    }
}

/// The Finder "File" menu from the reference: items, dimmed items, shortcuts.
fn file_menu() -> gtk::Box {
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("lunar-system-menu");
    menu.add_css_class("active-app-menu");
    menu.set_size_request(250, -1);
    let rows: [(&str, &str, bool); 18] = [
        ("New Finder Window", "⌘N", true),
        ("New Folder", "⇧⌘N", true),
        ("New Folder with Selection", "^⌘N", false),
        ("New Smart Folder", "", true),
        ("New Tab", "⌘T", true),
        ("Open", "⌘O", false),
        ("Open With", "›", false),
        ("Close Window", "⌘W", false),
        ("-", "", true),
        ("Get Info", "⌘I", true),
        ("Rename", "", false),
        ("Compress", "", false),
        ("Duplicate", "⌘D", false),
        ("Make Alias", "^⌘A", false),
        ("Quick Look", "⌘Y", false),
        ("-", "", true),
        ("Share…", "", false),
        ("Print", "⌘P", false),
    ];
    for (title, key, enabled) in rows {
        if title == "-" {
            let sep = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            sep.add_css_class("lunar-menu-separator");
            menu.append(&sep);
            continue;
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        row.append(&label);
        let shortcut = gtk::Label::new(Some(key));
        shortcut.set_opacity(0.55);
        row.append(&shortcut);
        let item = gtk::Button::new();
        item.set_child(Some(&row));
        item.add_css_class("lunar-menu-item");
        item.add_css_class("lunar-context-menu-item");
        item.set_sensitive(enabled);
        menu.append(&item);
    }
    menu
}

/// A Dock shelf with the bundled icons.
fn dock_shelf() -> gtk::Widget {
    let shelf = gtk::CenterBox::new();
    shelf.add_css_class("dock");
    shelf.set_halign(gtk::Align::Center);
    shelf.set_valign(gtk::Align::End);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_start(2);
    row.set_margin_end(2);
    for name in ["files", "browser", "terminal"] {
        let path = format!("assets/dock-icons/{name}.png");
        if let Ok(texture) = gdk::Texture::from_filename(&path) {
            let icon = gtk::Picture::for_paintable(&texture);
            icon.set_size_request(56, 56);
            row.append(&icon);
        }
    }
    let separator = gtk::Box::new(gtk::Orientation::Vertical, 0);
    separator.set_size_request(1, 48);
    separator.set_valign(gtk::Align::Center);
    separator.add_css_class("dock-separator");
    row.append(&separator);
    if let Ok(texture) = gdk::Texture::from_filename("assets/dock-icons/trash.png") {
        let icon = gtk::Picture::for_paintable(&texture);
        icon.set_size_request(56, 56);
        row.append(&icon);
    }
    shelf.set_center_widget(Some(&row));
    let glass = GlassSurface::wrap(&shelf, Material::dock());
    glass.add_css_class("dock-glass");
    glass.upcast()
}

#[allow(dead_code)]
fn _unused(_: Radius) {}
