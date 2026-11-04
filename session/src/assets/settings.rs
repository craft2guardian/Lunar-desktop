//! A lightweight System Settings window for shell configuration pages.
//!
//! Layout (like macOS): a translucent sidebar on the left that also holds the
//! window's close/minimise/maximise buttons, and a content area on the right
//! with a toolbar (back/forward + page title) above the page itself. There is
//! no separate title bar: the window draws its own frame ("client-side
//! decorations"), which is what lets the sidebar run all the way to the top.

use gtk::prelude::*;
use gtk::{gdk, glib};

const WINDOW_TITLE: &str = "System Settings";

/// One sidebar entry and the page it shows.
struct Page {
    /// Name of the page inside the `gtk::Stack`.
    name: &'static str,
    title: &'static str,
    /// PNG bytes, compiled into the binary so the app works from any folder.
    icon: &'static [u8],
}

/// To add a page: add an entry here and a matching arm in `build_page`.
const PAGES: [Page; 2] = [
    Page {
        name: "desktop-dock",
        title: "Desktop & Dock",
        icon: include_bytes!("../assets/settings-icons/desktop-dock.png"),
    },
    Page {
        name: "menu-bar",
        title: "Menu Bar",
        icon: include_bytes!("../assets/settings-icons/menu-bar.png"),
    },
];

fn sidebar_row(page: &Page) -> gtk::ListBoxRow {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

    let icon = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(page.icon)) {
        icon.set_paintable(Some(&texture));
    }
    icon.set_pixel_size(26);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("settings-sidebar-icon");

    let label = gtk::Label::new(Some(page.title));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);

    content.append(&icon);
    content.append(&label);

    let row = gtk::ListBoxRow::new();
    row.add_css_class("settings-sidebar-row");
    row.set_child(Some(&content));
    row
}

/// One line inside a group: title (and optional subtitle) on the left, the
/// control on the right.
fn settings_row(title: &str, subtitle: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(title));
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("settings-row-title");
    text.append(&title);
    if let Some(subtitle) = subtitle {
        let subtitle = gtk::Label::new(Some(subtitle));
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.add_css_class("settings-row-subtitle");
        text.append(&subtitle);
    }

    control.set_valign(gtk::Align::Center);
    row.append(&text);
    row.append(control);
    row
}

/// A white rounded card holding `rows`, with hairlines between them.
fn settings_group(rows: &[gtk::Box]) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add_css_class("settings-group");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            let line = gtk::Separator::new(gtk::Orientation::Horizontal);
            line.add_css_class("settings-row-separator");
            group.append(&line);
        }
        group.append(row);
    }
    group
}

/// The scrolling body of one page. The rows below are PLACEHOLDERS so you can
/// judge the styling; replace them with controls wired to real settings.
fn build_page(name: &str) -> gtk::ScrolledWindow {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 20);
    body.add_css_class("settings-page");

    match name {
        "desktop-dock" => {
            let size = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
            size.set_value(50.0);
            size.set_draw_value(false);
            size.set_size_request(200, -1);
            let magnification = gtk::Switch::new();
            let autohide = gtk::Switch::new();
            body.append(&settings_group(&[
                settings_row("Size", None, &size),
                settings_row("Magnification", None, &magnification),
                settings_row(
                    "Automatically hide and show the Dock",
                    None,
                    &autohide,
                ),
            ]));
        }
        _ => {
            let autohide = gtk::Switch::new();
            body.append(&settings_group(&[settings_row(
                "Automatically hide and show the menu bar",
                None,
                &autohide,
            )]));
        }
    }

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body));
    scroller
}

/// The pill-shaped back/forward control. Not wired up yet, so both are disabled.
fn navigation_pill() -> gtk::Box {
    let navigation = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    navigation.add_css_class("settings-navigation");
    navigation.set_valign(gtk::Align::Center);
    for (index, (icon_name, tooltip)) in [
        ("go-previous-symbolic", "Back"),
        ("go-next-symbolic", "Forward"),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            let divider = gtk::Separator::new(gtk::Orientation::Vertical);
            divider.add_css_class("settings-navigation-divider");
            navigation.append(&divider);
        }
        let button = gtk::Button::new();
        button.add_css_class("settings-navigation-button");
        button.set_tooltip_text(Some(tooltip));
        button.set_sensitive(false);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        button.set_child(Some(&icon));
        navigation.append(&button);
    }
    navigation
}

/// Presents the settings window, creating it on first use.
pub fn present(app: &gtk::Application) {
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|window| window.title().as_deref() == Some(WINDOW_TITLE))
    {
        window.present();
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(WINDOW_TITLE)
        .default_width(980)
        .default_height(680)
        .resizable(true)
        .css_classes(["sonoma-settings-window"])
        .build();
    // An empty title bar makes GTK draw the window frame itself instead of
    // KWin, so the sidebar can reach the top edge. The traffic-light buttons
    // live in the sidebar (below).
    let titlebar_stub = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    titlebar_stub.add_css_class("settings-titlebar-stub");
    window.set_titlebar(Some(&titlebar_stub));

    // ---- sidebar -------------------------------------------------------
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("settings-sidebar");
    sidebar.set_size_request(260, -1);
    sidebar.set_hexpand(false);

    let controls = gtk::WindowControls::new(gtk::PackType::Start);
    controls.set_decoration_layout(Some("close,minimize,maximize:"));
    controls.add_css_class("settings-window-controls");
    controls.set_halign(gtk::Align::Start);
    controls.set_valign(gtk::Align::Center);
    // Dragging the empty space around the buttons moves the window.
    let sidebar_top = gtk::WindowHandle::new();
    sidebar_top.add_css_class("settings-sidebar-top");
    sidebar_top.set_child(Some(&controls));
    sidebar.append(&sidebar_top);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search"));
    search.add_css_class("settings-search");
    sidebar.append(&search);

    let list = gtk::ListBox::new();
    list.add_css_class("settings-sidebar-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    for page in &PAGES {
        list.append(&sidebar_row(page));
    }
    let list_scroller = gtk::ScrolledWindow::new();
    list_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    list_scroller.set_vexpand(true);
    list_scroller.set_child(Some(&list));
    sidebar.append(&list_scroller);

    // ---- content -------------------------------------------------------
    let title = gtk::Label::new(Some(PAGES[0].title));
    title.set_halign(gtk::Align::Start);
    title.add_css_class("settings-page-title");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    toolbar.add_css_class("settings-toolbar");
    toolbar.append(&navigation_pill());
    toolbar.append(&title);
    let toolbar_handle = gtk::WindowHandle::new();
    toolbar_handle.set_child(Some(&toolbar));

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::None);
    for page in &PAGES {
        stack.add_named(&build_page(page.name), Some(page.name));
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("settings-content");
    content.set_hexpand(true);
    content.append(&toolbar_handle);
    content.append(&stack);

    // Selecting a sidebar row shows its page and updates the heading.
    let stack_for_rows = stack.downgrade();
    let title_for_rows = title.downgrade();
    list.connect_row_selected(move |_, row| {
        let Some(page) = row.and_then(|row| PAGES.get(row.index() as usize)) else {
            return;
        };
        if let Some(stack) = stack_for_rows.upgrade() {
            stack.set_visible_child_name(page.name);
        }
        if let Some(title) = title_for_rows.upgrade() {
            title.set_text(page.title);
        }
    });
    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.add_css_class("settings-root");
    // Clip the children to the rounded corners (CSS cannot do this in GTK).
    layout.set_overflow(gtk::Overflow::Hidden);
    layout.append(&sidebar);
    layout.append(&content);
    window.set_child(Some(&layout));

    crate::context_menu::attach_window_blur(&window);
    window.present();
}
