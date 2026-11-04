//! The only CSS the shell needs: strip the toolkit's default window chrome so
//! the surfaces show exactly what we paint.

use gtk::gdk;

const CSS: &str = "
window.sonoma-wallpaper,
window.sonoma-menubar {
    border: none;
    border-radius: 0;
    box-shadow: none;
    outline: none;
}

/* Only visible for the instant before the first frame is painted. */
window.sonoma-wallpaper { background-color: #2f3573; }

/* The bar paints its own glass; keep the window itself see-through. */
window.sonoma-menubar { background-color: transparent; }

/* ---- System Settings -------------------------------------------------
   The window draws its own frame (see settings.rs). Everything is clipped to
   .settings-root so the corners are round; the 1px shadow is just an outline
   because a big drop shadow would also be covered by the compositor blur. */
window.sonoma-settings-window {
    background-color: transparent;
    border-radius: 12px;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.22);
    color: #1d1d1f;
    font-family: \"Inter\", \"SF Pro Text\", \"Noto Sans\", sans-serif;
    font-size: 13px;
}
.settings-root {
    border-radius: 12px;
}
/* GTK adds a default title-bar height; the real controls live in the sidebar. */
.settings-titlebar-stub {
    min-height: 0;
    margin: 0;
    padding: 0;
    border: none;
    background: none;
    box-shadow: none;
}

/* Sidebar: tinted over the blurred backdrop. */
.settings-sidebar {
    background-color: rgba(244, 244, 247, 0.78);
    border-right: 1px solid rgba(0, 0, 0, 0.08);
}
.settings-sidebar-top {
    min-height: 52px;
}
.settings-window-controls {
    margin-left: 20px;
}
/* Traffic lights: plain coloured dots, glyphs hidden. */
.settings-window-controls button {
    min-width: 12px;
    min-height: 12px;
    padding: 0;
    margin: 0 4px 0 0;
    border: 1px solid rgba(0, 0, 0, 0.14);
    border-radius: 50%;
    box-shadow: none;
    background-image: none;
}
.settings-window-controls button image {
    opacity: 0;
    -gtk-icon-size: 8px;
}
.settings-window-controls button:hover image {
    opacity: 0.55;
}
.settings-window-controls button.close { background-color: #ff5f57; }
.settings-window-controls button.minimize { background-color: #febc2e; }
.settings-window-controls button.maximize { background-color: #28c840; }
.settings-window-controls button:backdrop {
    background-color: rgba(0, 0, 0, 0.14);
}

.settings-search {
    min-height: 28px;
    margin: 0 16px 10px;
    padding: 0 8px;
    border: 1px solid rgba(0, 0, 0, 0.10);
    border-radius: 8px;
    outline: none;
    box-shadow: none;
    background-color: rgba(0, 0, 0, 0.055);
    color: #1d1d1f;
}
.settings-search:focus-within {
    border-color: rgba(10, 132, 255, 0.85);
    background-color: #ffffff;
    box-shadow: 0 0 0 3px rgba(10, 132, 255, 0.28);
}

list.settings-sidebar-list {
    padding: 0 10px;
    background: transparent;
}
row.settings-sidebar-row {
    min-height: 34px;
    margin: 1px 0;
    padding: 0 8px;
    border-radius: 8px;
    outline: none;
    background: transparent;
    color: #1d1d1f;
}
row.settings-sidebar-row:hover {
    background-color: rgba(0, 0, 0, 0.055);
}
row.settings-sidebar-row:selected,
row.settings-sidebar-row:selected:hover {
    background-color: #0a84ff;
    color: #ffffff;
}
.settings-sidebar-icon {
    border-radius: 7px;
}

/* Content side. */
.settings-content {
    background-color: #f5f5f7;
}
.settings-toolbar {
    min-height: 52px;
    padding: 0 20px;
}
.settings-page-title {
    font-size: 20px;
    font-weight: 700;
    color: #1d1d1f;
}
.settings-navigation {
    min-height: 32px;
    border: 1px solid rgba(0, 0, 0, 0.09);
    border-radius: 16px;
    background-color: #ffffff;
}
button.settings-navigation-button {
    min-width: 36px;
    min-height: 30px;
    padding: 0;
    border: none;
    border-radius: 15px;
    background: transparent;
    color: #3a3a3c;
    box-shadow: none;
}
button.settings-navigation-button:disabled {
    color: rgba(60, 60, 67, 0.30);
}
.settings-navigation-divider {
    min-width: 1px;
    margin: 7px 0;
    background-color: rgba(0, 0, 0, 0.12);
}
.settings-page {
    padding: 4px 20px 24px;
}

/* Grouped rows: white rounded cards. */
.settings-group {
    background-color: #ffffff;
    border: 1px solid rgba(0, 0, 0, 0.07);
    border-radius: 10px;
}
.settings-row {
    min-height: 26px;
    padding: 9px 14px;
}
.settings-row-title {
    font-weight: 500;
}
.settings-row-subtitle {
    font-size: 12px;
    color: rgba(60, 60, 67, 0.60);
}
/* Switches: macOS pill with a white knob, accent colour when on. */
.settings-row switch {
    min-width: 38px;
    min-height: 22px;
    margin: 0;
    padding: 0;
    border: none;
    border-radius: 11px;
    outline: none;
    background-color: rgba(120, 120, 128, 0.28);
    box-shadow: none;
}
.settings-row switch:checked {
    background-color: #0a84ff;
}
.settings-row switch slider {
    min-width: 18px;
    min-height: 18px;
    margin: 2px;
    border: none;
    border-radius: 50%;
    background-color: #ffffff;
    background-image: none;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.30);
}
.settings-row switch image {
    opacity: 0;
}
.settings-row-separator {
    min-height: 1px;
    margin: 0 14px;
    background-color: rgba(0, 0, 0, 0.08);
}

/* Dock: the window is see-through; the rounded translucent shelf is .dock.
   Its margins leave room for the soft shadow and the running indicators. */
window.sonoma-dock {
    background-color: transparent;
    border: none;
    box-shadow: none;
}
.dock {
    margin: 24px 24px 16px 24px;
    padding: 8px 12px;
    background-color: rgba(255, 255, 255, 0.22);
    border: 1px solid rgba(255, 255, 255, 0.38);
    border-radius: 22px;
    box-shadow: 0 3px 14px rgba(0, 0, 0, 0.20);
}
.dock-separator {
    background-color: rgba(255, 255, 255, 0.42);
}
.dock-item {
    padding: 0;
    min-width: 0;
    min-height: 0;
    border: none;
    border-radius: 24%;
    color: white;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
}
.dock-item.dock-icon {
    background: none;
    box-shadow: none;
    border-radius: 22%;
}
.dock-item.dock-icon:hover {
    background-color: rgba(255, 255, 255, 0.12);
}
.dock-item:active {
    background-color: rgba(0, 0, 0, 0.30);
}
.dock-item:active image,
.dock-item:active label {
    opacity: 0.68;
}
.dock-running-indicator {
    min-width: 6px;
    min-height: 4px;
    background-color: rgba(255, 255, 255, 0.9);
    border-radius: 50%;
}
.dock-item-minimized .dock-running-indicator {
    opacity: 0.58;
}
/* Neutral lettered tile when no icon is available. */
.dock-item.dock-placeholder {
    background-image: linear-gradient(#b3b8c2, #8a909c);
    font-size: 22px;
    font-weight: 600;
}
button.sonoma-menu-button,
button.sonoma-menu-button:hover,
button.sonoma-menu-button:active,
button.sonoma-menu-button:checked {
    padding: 0;
    border: none;
    border-radius: 0;
    outline: none;
    background: transparent;
    box-shadow: none;
}
popover.sonoma-menu-popover {
    padding: 0;
    border: none;
    outline: none;
    background: transparent;
    box-shadow: none;
    border-radius: 12px;
}
popover.sonoma-menu-popover > contents {
    padding: 0;
    border: none;
    outline: none;
    background: transparent;
    box-shadow: none;
}
.sonoma-system-menu {
    min-width: 220px;
    padding: 5px 6px;
    background-color: rgba(137, 192, 232, 0.54);
    border: none;
    border-radius: 12px;
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.22);
}
button.sonoma-menu-item {
    min-height: 25px;
    padding: 1px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: #171717;
    box-shadow: none;
    transition: background-color 120ms ease-out, color 120ms ease-out;
}
button.sonoma-menu-item:hover,
button.sonoma-menu-item:active {
    background-color: rgba(0, 122, 255, 0.94);
    color: white;
}
button.sonoma-menu-item:disabled {
    color: rgba(23, 23, 23, 0.48);
}
.sonoma-menu-separator {
    min-height: 1px;
    margin: 3px 6px;
    background-color: rgba(0, 0, 0, 0.16);
}
.sonoma-system-menu.dock-context-menu {
    min-width: 126px;
    padding: 3px;
}
.sonoma-system-menu.dock-context-submenu {
    min-width: 164px;
    padding: 3px;
}
button.sonoma-context-menu-item {
    min-height: 23px;
    padding: 0 6px;
    border-radius: 5px;
    font-size: 13px;
}
.sonoma-context-menu-arrow {
    margin-left: 10px;
}
.sonoma-context-menu-separator {
    margin: 2px 5px;
}
";

pub fn install(display: &gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
