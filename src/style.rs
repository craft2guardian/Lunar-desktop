//! The only CSS the shell needs: strip the toolkit's default window chrome so
//! the surfaces show exactly what we paint.

use gtk::gdk;

const CSS: &str = "
window.lunar-wallpaper,
window.lunar-menubar {
    border: none;
    border-radius: 0;
    box-shadow: none;
    outline: none;
}

/* Only visible for the instant before the first frame is painted. */
window.lunar-wallpaper { background-color: #2f3573; }

/* The tint and hairline are drawn by GlassStrip; KWin paints the blur. */
window.lunar-menubar {
    background-color: transparent;
}

/* ---- System Settings -------------------------------------------------
   The window draws its own frame (see settings.rs). Everything is clipped to
   .settings-root so the corners are round; the 1px shadow is just an outline
   because a big drop shadow would also be covered by the compositor blur. */
window.lunar-settings-window {
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
    background-color: rgba(236, 240, 248, 0.78);
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.34), rgba(255, 255, 255, 0.06));
    box-shadow: inset -1px 0 0 rgba(255, 255, 255, 0.55), inset 0 1px 0 rgba(255, 255, 255, 0.65);
    border-right: 1px solid rgba(0, 0, 0, 0.07);
}
.settings-sidebar-top {
    min-height: 52px;
}
.settings-window-controls {
    margin-left: 20px;
}
/* Traffic lights: plain coloured dots, glyphs hidden. */
.settings-window-controls button {
    min-width: 18px;
    min-height: 18px;
    padding: 0;
    margin: 0 0px 0 0;
    border: 1px solid rgba(0, 0, 0, 0.14);
    border-radius: 50%;
    box-shadow: none;
    background-image: none;
}
.settings-window-controls button.settings-window-control {
    min-width: 14px;
    min-height: 14px;
    padding: 0;
}
.settings-window-controls button image {
    opacity: 0;
    -gtk-icon-size: 8px;
}
.settings-window-controls button:hover image {
    opacity: 0.55;
}
/* Traffic lights: small glass beads with a lit upper edge. */
.settings-window-controls button.close { background-color: #ff5f57; }
.settings-window-controls button.minimize { background-color: #febc2e; }
.settings-window-controls button.maximize { background-color: #28c840; }
.settings-window-controls button.close,
.settings-window-controls button.minimize,
.settings-window-controls button.maximize {
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.34), rgba(255, 255, 255, 0) 60%);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.55), 0 0.5px 1.5px rgba(0, 0, 0, 0.18);
}
.settings-window-controls button:backdrop {
    background-color: rgba(0, 0, 0, 0.14);
}

.settings-search {
    min-height: 28px;
    margin: 0 16px 10px;
    padding: 0 8px;
    border: 1px solid rgba(0, 0, 0, 0.08);
    border-radius: 14px;
    outline: none;
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.07), 0 1px 0 rgba(255, 255, 255, 0.55);
    background-color: rgba(0, 0, 0, 0.05);
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
    min-height: 30px;
    margin: 1px 0;
    padding: 0 8px;
    border-radius: 7px;
    font-size: 13px;
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
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.20), rgba(255, 255, 255, 0));
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.40),
                inset 0 0 0 0.5px rgba(190, 225, 255, 0.45),
                0 1px 3px rgba(0, 70, 190, 0.30);
    color: #ffffff;
}
.settings-sidebar-icon {
    border-radius: 7px;
}
image.settings-sidebar-icon {
    color: #727780;
}
.settings-icon-dark {
    opacity: 0;
}
.dark-mode .settings-icon-light {
    opacity: 0;
}
.dark-mode .settings-icon-dark {
    opacity: 1;
}

/* Content side. */
.settings-content {
    background-color: #ffffff;
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
/* General (and similar) page hero: large icon, title, description. */
.settings-page-hero {
    padding: 8px 12px 4px;
}
.settings-page-hero-icon {
    color: #6e6e73;
    margin-bottom: 4px;
}
.settings-page-hero-title {
    font-size: 22px;
    font-weight: 700;
    color: #1d1d1f;
}
.settings-page-hero-description {
    font-size: 13px;
    line-height: 1.35;
    color: rgba(60, 60, 67, 0.70);
}
.settings-nav-row {
    min-height: 36px;
}
.settings-nav-icon {
    color: #0a84ff;
}
.settings-nav-chevron {
    color: rgba(60, 60, 67, 0.35);
}
.settings-section-heading {
    margin: 10px 4px 0;
    font-size: 13px;
    font-weight: 700;
    color: #1d1d1f;
}
.settings-section-heading:first-child {
    margin-top: 0;
}
.settings-slider-caption {
    font-size: 11px;
    color: rgba(60, 60, 67, 0.60);
}
/* Slider: thin track, accent fill, white knob. */
.settings-row scale trough {
    min-height: 4px;
    border: none;
    border-radius: 2px;
    background-color: rgba(120, 120, 128, 0.28);
}
.settings-row scale highlight {
    border: none;
    border-radius: 2px;
    background-color: #0a84ff;
}
.settings-row scale slider {
    min-width: 18px;
    min-height: 18px;
    margin: -8px;
    border: 0.5px solid rgba(0, 0, 0, 0.16);
    border-radius: 50%;
    background-color: #ffffff;
    background-image: none;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.28);
}
.settings-row scale:disabled {
    opacity: 0.45;
}
/* Check boxes. */
.settings-row checkbutton {
    padding: 0;
    outline: none;
}
.settings-row checkbutton check {
    min-width: 15px;
    min-height: 15px;
    margin-right: 5px;
    border: 1px solid rgba(0, 0, 0, 0.28);
    border-radius: 4px;
    background-color: #ffffff;
    background-image: none;
    box-shadow: none;
    color: #ffffff;
}
.settings-row checkbutton check:checked {
    border-color: #0a84ff;
    background-color: #0a84ff;
}
/* Pop-up button with the blue stepper. */
button.settings-popup {
    min-height: 22px;
    padding: 1px 2px 1px 10px;
    border: 1px solid rgba(0, 0, 0, 0.12);
    border-radius: 6px;
    background-color: #ffffff;
    background-image: none;
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.06);
    color: #1d1d1f;
    outline: none;
}
.settings-popup-value {
    font-size: 13px;
}
.settings-popup-stepper {
    min-width: 17px;
    min-height: 17px;
    border-radius: 4px;
    background-color: #0a84ff;
    color: #ffffff;
}

/* Grouped rows: white rounded cards. */
.settings-group {
    background-color: #f5f5f7;
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
.settings-row switch,
.wifi-menu switch {
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
.settings-row switch:checked,
.wifi-menu switch:checked {
    background-color: #0a84ff;
}
.settings-row switch slider,
.wifi-menu switch slider {
    min-width: 18px;
    min-height: 18px;
    margin: 2px;
    border: none;
    border-radius: 50%;
    background-color: #ffffff;
    background-image: linear-gradient(to bottom, #ffffff, #f1f3f7);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.30), inset 0 1px 0 rgba(255, 255, 255, 1);
}
.settings-row switch image,
.wifi-menu switch image {
    opacity: 0;
}
.settings-row-separator {
    min-height: 1px;
    margin: 0 14px;
    background-color: rgba(0, 0, 0, 0.08);
}

/* Dock: the window is see-through; the rounded translucent shelf is .dock.
   Its margins leave room for the soft shadow and the running indicators. */
window.lunar-dock {
    background-color: transparent;
    border: none;
    box-shadow: none;
}
/* The shelf material (fill, rim, specular, shadow) is painted by
   liquid_glass::GlassSurface. The margin that reserves room for its shadow and
   the corner radius are generated from glass_config.rs (see glass_config::css). */
.dock {
    margin: 0;
    padding: 6px 9px;
    background: none;
    border: none;
    box-shadow: none;
}
.dock-separator {
    background-color: rgba(255, 255, 255, 0.42);
    box-shadow: 0 0 0 0.5px rgba(0, 0, 0, 0.10);
    border-radius: 1px;
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
/* No hover highlight on Dock items. */
.dock-item.dock-icon:hover {
    background: none;
}
.dock-item:active {
    background-color: rgba(0, 0, 0, 0.30);
}
.dock-item:active image,
.dock-item:active label {
    opacity: 0.68;
}
.dock-running-indicator {
    min-width: 4px;
    min-height: 4px;
    background-color: rgba(255, 255, 255, 0.92);
    box-shadow: 0 0 3px rgba(255, 255, 255, 0.55), 0 0 0 0.5px rgba(0, 0, 0, 0.18);
    border-radius: 50%;
}
.dock-item-minimized .dock-running-indicator {
    opacity: 1;
}
/* Minimised windows, shown beside the Trash when the Minimize-into-icon
   setting is off: a small window with the app icon as a badge. */
.dock-item.dock-minimized-window {
    background: none;
    box-shadow: none;
    border-radius: 6px;
}
.dock-minimized-thumb {
    margin: 2px;
    background-image: linear-gradient(#f7f8fa, #d8dbe1);
    border: 1px solid rgba(0, 0, 0, 0.28);
    border-radius: 6px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.30);
}
.dock-minimized-picture {
    border-radius: 4px;
}
.dock-minimized-titlebar {
    padding: 3px 5px;
    background-color: rgba(0, 0, 0, 0.10);
    border-radius: 5px 5px 0 0;
}
.dock-minimized-dot {
    min-width: 4px;
    min-height: 4px;
    background-color: rgba(0, 0, 0, 0.35);
    border-radius: 50%;
}
.dock-minimized-letter {
    min-width: 18px;
    min-height: 18px;
    font-size: 11px;
    font-weight: 600;
    background-image: linear-gradient(#b3b8c2, #8a909c);
    border-radius: 22%;
}
.dark-mode .dock-minimized-thumb {
    background-image: linear-gradient(#4b4e56, #34363c);
    border-color: rgba(255, 255, 255, 0.22);
}
.dark-mode .dock-minimized-titlebar {
    background-color: rgba(255, 255, 255, 0.08);
}
.dark-mode .dock-minimized-dot {
    background-color: rgba(255, 255, 255, 0.42);
}
/* Neutral lettered tile when no icon is available. */
.dock-item.dock-placeholder {
    background-image: linear-gradient(#b3b8c2, #8a909c);
    font-size: 22px;
    font-weight: 600;
}
.dock-item.dock-placeholder:hover {
    background-image: linear-gradient(#b3b8c2, #8a909c);
}
menubutton.lunar-menu-button,
menubutton.lunar-menu-button > button,
menubutton.lunar-menu-button > button:hover,
menubutton.lunar-menu-button > button:active,
menubutton.lunar-menu-button > button:checked,
menubutton.lunar-menu-button > button:backdrop {
    padding: 0;
    min-height: 26px;
    min-width: 32px;
    border: none;
    border-radius: 0;
    outline: none;
    background: transparent;
    background-image: none;
    box-shadow: none;
}
/* The open-menu capsule (menubutton.lunar-menu-button.menu-open) is generated
   from glass_config::MENU_CHROME. */
.lunar-kde-logo-dark {
    opacity: 0;
}
.lunar-kde-logo-light.dark-mode {
    opacity: 0;
}
.lunar-kde-logo-dark.dark-mode {
    opacity: 1;
}
.lunar-active-app {
    margin-left: 0;
    font-size: 13px;
    font-weight: 800;
    color: #ffffff;
}
.lunar-active-app.dark-mode {
    color: #111113;
}
menubutton.lunar-active-app-button,
menubutton.lunar-active-app-button > button,
menubutton.lunar-active-app-button > button:hover,
menubutton.lunar-active-app-button > button:active,
menubutton.lunar-active-app-button > button:checked {
    min-width: 0;
    min-height: 28px;
    padding: 0 3px;
    border: none;
    border-radius: 0;
    outline: none;
    background: transparent;
    background-image: none;
    box-shadow: none;
}
popover.lunar-menu-popover {
    padding: 0;
    border: none;
    outline: none;
    background: transparent;
    box-shadow: none;
}
/* Corner radius and the (invisible) shadow-room box-shadow of popovers are
   generated from glass_config.rs. */
popover.lunar-menu-popover > contents {
    padding: 0;
    border: none;
    outline: none;
    background: transparent;
}
.lunar-system-menu {
    min-width: 220px;
    padding: 6px 6px;
    background: none;
    border: none;
    box-shadow: none;
    color: rgba(255, 255, 255, 0.96);
    text-shadow: 0 0.5px 1px rgba(0, 20, 60, 0.28);
}
button.lunar-menu-item {
    min-height: 25px;
    padding: 1px 9px;
    border: none;
    border-radius: 7px;
    background: transparent;
    background-image: none;
    color: rgba(255, 255, 255, 0.96);
    box-shadow: none;
}
/* Hover: a small accent-glass capsule with its own lit rim (background, rim
   and the fade transition come from glass_config::MENU_CHROME). */
button.lunar-menu-item:hover,
button.lunar-menu-item:active {
    color: #ffffff;
    text-shadow: none;
}
button.lunar-menu-item:disabled {
    color: rgba(255, 255, 255, 0.36);
}
.lunar-menu-separator {
    min-height: 1px;
    margin: 4px 6px;
    background-color: rgba(255, 255, 255, 0.20);
}
.lunar-system-menu.dock-context-menu {
    min-width: 180px;
    padding: 3px;
}
.lunar-system-menu.dock-context-submenu {
    min-width: 190px;
    padding: 3px;
}
/* The little arrow pointing from the Dock menu at its icon. */
popover.lunar-menu-popover.dock-context-popover > arrow,
popover.lunar-menu-popover.dock-label-popover > arrow {
    min-width: 14px;
    min-height: 14px;
    border: none;
    box-shadow: none;
}
/* App-name bubble shown above a Dock icon on hover. */
.lunar-system-menu.dock-label {
    min-width: 0;
    padding: 4px 11px;
    font-size: 13px;
    color: #ffffff;
}
.lunar-system-menu.active-app-menu {
    min-width: 200px;
    padding: 3px;
}
.active-app-menu button.lunar-menu-item {
    min-height: 23px;
    padding: 0 7px;
    font-size: 12px;
}
button.lunar-context-menu-item {
    min-height: 23px;
    padding: 0 6px;
    border-radius: 5px;
    font-size: 13px;
}
.lunar-context-menu-arrow {
    margin-left: 10px;
}
.lunar-context-menu-separator {
    margin: 2px 5px;
}
.active-app-menu searchentry.lunar-help-search {
    min-height: 22px;
    margin: 2px 3px 4px 3px;
    padding: 0 6px;
    font-size: 12px;
    border-radius: 6px;
}

/* Dark appearance is opt-in and follows the saved application preference. */
window.lunar-settings-window.dark-mode {
    background-color: transparent;
    color: #f2f2f4;
}
.dark-mode .settings-sidebar {
    background-color: rgba(38, 38, 42, 0.84);
    border-right-color: rgba(255, 255, 255, 0.07);
}
.dark-mode .settings-content,
.dark-mode .settings-page {
    background-color: #1e1e21;
}
.dark-mode .settings-page-title,
.dark-mode .settings-row-title,
.dark-mode row.settings-sidebar-row {
    color: #f5f5f7;
}
.dark-mode .settings-row-subtitle {
    color: rgba(235, 235, 245, 0.62);
}
image.settings-sidebar-icon.dark-mode {
    color: #e6e6eb;
}
.dark-mode .settings-search {
    background-color: rgba(255, 255, 255, 0.075);
    border-color: rgba(255, 255, 255, 0.10);
    color: #f5f5f7;
}
.dark-mode .settings-search:focus-within {
    background-color: rgba(57, 57, 61, 0.98);
}
.dark-mode .settings-section-heading {
    color: #f5f5f7;
}
.dark-mode .settings-page-hero-icon {
    color: #aeaeb2;
}
.dark-mode .settings-page-hero-title {
    color: #f5f5f7;
}
.dark-mode .settings-page-hero-description {
    color: rgba(235, 235, 245, 0.62);
}
.dark-mode .settings-nav-icon {
    color: #0a84ff;
}
.dark-mode .settings-nav-chevron {
    color: rgba(235, 235, 245, 0.35);
}
.dark-mode .settings-slider-caption {
    color: rgba(235, 235, 245, 0.55);
}
.dark-mode .settings-row checkbutton check {
    border-color: rgba(255, 255, 255, 0.30);
    background-color: rgba(255, 255, 255, 0.08);
}
.dark-mode .settings-row checkbutton check:checked {
    border-color: #0a84ff;
    background-color: #0a84ff;
}
.dark-mode button.settings-popup {
    background-color: rgba(255, 255, 255, 0.12);
    border-color: rgba(255, 255, 255, 0.10);
    color: #f5f5f7;
}
.dark-mode .settings-row scale trough {
    background-color: rgba(255, 255, 255, 0.22);
}
.dark-mode .settings-group {
    background-color: #2b2b2f;
    border-color: rgba(255, 255, 255, 0.06);
}
.settings-wallpaper-preview {
    border: none;
    border-radius: 11px;
    background: transparent;
    box-shadow: none;
}
.settings-wallpaper-caption {
    font-weight: 600;
}
.dark-mode .settings-row-separator,
.dark-mode .settings-navigation-divider {
    background-color: rgba(255, 255, 255, 0.12);
}
.dark-mode .settings-navigation {
    background-color: #343438;
    border-color: rgba(255, 255, 255, 0.08);
}
.dark-mode button.settings-navigation-button {
    color: #e5e5ea;
}
.dark-mode button.settings-navigation-button:disabled {
    color: rgba(235, 235, 245, 0.28);
}
.dark-mode row.settings-sidebar-row:hover {
    background-color: rgba(255, 255, 255, 0.08);
}
.dark-mode row.settings-sidebar-row:selected,
.dark-mode row.settings-sidebar-row:selected:hover {
    background-color: #0a84ff;
    color: #ffffff;
}
.dock-separator.dark-mode {
    background-color: rgba(255, 255, 255, 0.30);
}
/* ---- Wallpaper picker (settings.rs) and desktop context menu ------------ */
.lunar-system-menu.desktop-context-menu {
    min-width: 190px;
    padding: 3px;
}
.wallpaper-grid flowboxchild {
    padding: 0;
    background: transparent;
    outline: none;
}
button.wallpaper-thumb {
    min-width: 0;
    min-height: 0;
    padding: 0;
    border: 3px solid transparent;
    border-radius: 10px;
    background: transparent;
    background-image: none;
    box-shadow: none;
    outline: none;
}
button.wallpaper-thumb:hover {
    background: transparent;
}
button.wallpaper-thumb.wallpaper-thumb-selected {
    border-color: #0a84ff;
}
button.wallpaper-add {
    min-height: 22px;
    padding: 1px 12px;
    border: 1px solid rgba(0, 0, 0, 0.12);
    border-radius: 6px;
    background-color: #ffffff;
    background-image: none;
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.06);
    color: #1d1d1f;
}
.dark-mode button.wallpaper-add {
    background-color: rgba(255, 255, 255, 0.12);
    border-color: rgba(255, 255, 255, 0.10);
    color: #f5f5f7;
}
/* ---- Wi-Fi menu (see menubar.rs `wifi_menu`) ----------------------------- */
.lunar-system-menu.wifi-menu {
    min-width: 0;
    padding: 4px;
}
.wifi-menu .wifi-title {
    font-size: 12px;
    font-weight: 700;
    color: #ffffff;
}
.wifi-menu .lunar-menu-separator {
    margin: 3px 3px;
}
.wifi-menu .wifi-section-title {
    margin: 3px 6px 3px 6px;
    font-size: 12px;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.62);
}
.wifi-menu .wifi-status {
    margin: 0 6px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.62);
}
.wifi-menu button.lunar-menu-item {
    min-height: 24px;
    padding: 0 6px;
    font-size: 12px;
}
.wifi-dropdown {
    padding: 0 0 0 6px;
}
/* The connected network: tall row, grey hover instead of the blue highlight. */
.wifi-menu button.wifi-current-item {
    min-height: 32px;
}
.wifi-menu button.wifi-current-item:hover,
.wifi-menu button.wifi-current-item:active {
    background-image: none;
    background-color: rgba(255, 255, 255, 0.14);
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.22);
    color: #ffffff;
}
.wifi-badge {
    min-width: 28px;
    min-height: 28px;
    border-radius: 14px;
    background-color: #0a84ff;
}
.wifi-badge-icon {
    color: #ffffff;
}
.wifi-lock {
    color: rgba(255, 255, 255, 0.62);
}
.wifi-menu button.lunar-menu-item:hover .wifi-lock {
    color: #ffffff;
}
.wifi-menu button.wifi-current-item:hover .wifi-lock {
    color: rgba(255, 255, 255, 0.62);
}
/* Other Networks row: bold, muted heading style like Preferred Network. */
.wifi-menu button.wifi-heading-item {
    font-weight: 700;
    color: rgba(255, 255, 255, 0.62);
}
.wifi-menu button.wifi-heading-item:hover,
.wifi-menu button.wifi-heading-item:active {
    color: #ffffff;
}
.wifi-menu button.wifi-heading-item:disabled {
    color: rgba(255, 255, 255, 0.32);
}
.wifi-chevron {
    font-size: 14px;
    font-weight: 400;
}
.dark-mode .wifi-menu .wifi-title {
    color: #f5f5f7;
}
.dark-mode .wifi-menu .wifi-section-title,
.dark-mode .wifi-menu .wifi-status,
.dark-mode .wifi-menu button.wifi-heading-item {
    color: rgba(235, 235, 245, 0.62);
}
.dark-mode .wifi-menu button.wifi-heading-item:hover,
.dark-mode .wifi-menu button.wifi-heading-item:active {
    color: #ffffff;
}
.dark-mode .wifi-menu button.wifi-heading-item:disabled {
    color: rgba(235, 235, 245, 0.32);
}
.dark-mode .wifi-menu button.wifi-current-item:hover,
.dark-mode .wifi-menu button.wifi-current-item:active {
    background-color: rgba(255, 255, 255, 0.10);
    color: #f5f5f7;
}
.dark-mode .wifi-lock,
.dark-mode .wifi-menu button.wifi-current-item:hover .wifi-lock {
    color: rgba(245, 245, 247, 0.62);
}
.dark-mode .wifi-menu button.lunar-menu-item:hover .wifi-lock {
    color: #ffffff;
}
/* ---- About This System (see about.rs) ---------------------------------- */
/* KWin's blur strength is a global setting the app can't change, so the
   About window gets a mostly opaque tint to tame how much backdrop shows. */
.lunar-about-window .settings-root {
    background-color: rgba(246, 246, 248, 0.88);
}
.lunar-about-window.dark-mode .settings-root {
    background-color: rgba(40, 40, 44, 0.90);
}
.about-top {
    min-height: 40px;
}
.about-window-controls {
    margin-left: 20px;
}
.lunar-about-window .settings-window-controls button.about-inert {
    background-color: rgba(0, 0, 0, 0.14);
}
.about-content {
    padding: 0 28px 22px;
}
.about-monitor {
    background-color: #0b0b0d;
    border: 3px solid #1c1c1e;
    border-radius: 8px;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
}
.about-monitor-neck {
    background-color: #b9bbc1;
    border-radius: 0 0 2px 2px;
}
.about-monitor-foot {
    background-color: #a7a9af;
    border-radius: 3px;
}
.about-model {
    margin-top: 16px;
    font-size: 20px;
    font-weight: 700;
}
.about-maker {
    margin-top: 2px;
    font-size: 12px;
    color: rgba(60, 60, 67, 0.6);
}
.about-info {
    margin-top: 18px;
}
.about-info-label {
    font-size: 12px;
    font-weight: 600;
    color: rgba(60, 60, 67, 0.6);
}
.about-info-value {
    font-size: 12px;
}
button.about-more-info {
    min-height: 24px;
    margin-top: 20px;
    padding: 0 14px;
    border: 1px solid rgba(0, 0, 0, 0.14);
    border-radius: 6px;
    background-image: none;
    background-color: rgba(255, 255, 255, 0.92);
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.08);
    color: #1d1d1f;
    font-size: 12px;
}
button.about-more-info:active {
    background-color: rgba(0, 0, 0, 0.08);
}
.about-footer {
    margin-top: 20px;
    font-size: 10px;
    color: rgba(60, 60, 67, 0.5);
}
.dark-mode .about-maker,
.dark-mode .about-info-label {
    color: rgba(235, 235, 245, 0.62);
}
.dark-mode .about-footer {
    color: rgba(235, 235, 245, 0.42);
}
.dark-mode .about-model,
.dark-mode .about-info-value {
    color: #f5f5f7;
}
.lunar-about-window.dark-mode .settings-window-controls button.about-inert {
    background-color: rgba(255, 255, 255, 0.16);
}
.dark-mode button.about-more-info {
    background-color: rgba(99, 99, 102, 0.9);
    border-color: rgba(255, 255, 255, 0.10);
    color: #f5f5f7;
}

/* ---- Menu bar titles next to the app name (File, ...) ------------------ */
.lunar-menubar-item {
    margin-left: 0;
    font-size: 13px;
    font-weight: 500;
    color: #ffffff;
}
.lunar-menubar-item.dark-mode {
    color: #111113;
}
/* The bar colours follow the wallpaper behind the bar (menubar::set_tone puts
   one of these two classes on the bar window), not the Dark Mode setting. */
.lunar-menubar.bar-light-text .lunar-menubar-item,
.lunar-menubar.bar-light-text .lunar-active-app {
    color: #ffffff;
}
.lunar-menubar.bar-dark-text .lunar-menubar-item,
.lunar-menubar.bar-dark-text .lunar-active-app {
    color: #111113;
}
.lunar-menubar.bar-light-text .lunar-kde-logo-light {
    opacity: 1;
}
.lunar-menubar.bar-light-text .lunar-kde-logo-dark {
    opacity: 0;
}
.lunar-menubar.bar-dark-text .lunar-kde-logo-light {
    opacity: 0;
}
.lunar-menubar.bar-dark-text .lunar-kde-logo-dark {
    opacity: 1;
}
menubutton.lunar-menubar-item-button,
menubutton.lunar-menubar-item-button > button,
menubutton.lunar-menubar-item-button > button:hover,
menubutton.lunar-menubar-item-button > button:active,
menubutton.lunar-menubar-item-button > button:checked {
    min-width: 0;
    min-height: 22px;
    padding: 0 8px;
    border: none;
    border-radius: 5px;
    outline: none;
    background: transparent;
    background-image: none;
    box-shadow: none;
}
/* An open bar menu: a clear glass capsule behind the title. */
menubutton.lunar-menubar-item-button > button:checked {
    background-color: rgba(255, 255, 255, 0.20);
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.18), rgba(255, 255, 255, 0.03));
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.50),
                inset 0 1px 0 rgba(255, 255, 255, 0.40),
                0 1px 4px rgba(0, 30, 90, 0.16);
    border-radius: 11px;
}


/* ---- Sidebar like macOS System Settings: account block and group gaps --- */
.settings-sidebar-gap {
    min-height: 14px;
}
.settings-account {
    margin: 2px 10px 10px;
    padding: 6px 8px;
}
.settings-account-avatar {
    border-radius: 50%;
    background-image: linear-gradient(#b4b9c4, #838995);
    color: #ffffff;
    font-size: 19px;
    font-weight: 600;
}
.settings-account-name {
    font-size: 13px;
    font-weight: 600;
    color: #1d1d1f;
}
.settings-account-caption {
    font-size: 11px;
    color: rgba(60, 60, 67, 0.62);
}
.dark-mode .settings-account-name {
    color: #f5f5f7;
}
.dark-mode .settings-account-caption {
    color: rgba(235, 235, 245, 0.62);
}

/* Menu bar clock on the right. */
.lunar-menubar-clock {
    font-size: 13px;
    font-weight: 500;
}
/* ---- Control Centre (see control_center.rs) ------------------------------
   No panel background. Every control sits on its own Liquid Glass surface
   (liquid_glass::GlassSurface paints fill, meniscus, specular lines, rim and
   shadow, and animates hover/press); this CSS only lays out the content. */
.lunar-system-menu.cc-panel,
.dark-mode .lunar-system-menu.cc-panel {
    min-width: 0;
    padding: 12px;
    border-radius: 26px;
    background: none;
    box-shadow: none;
    text-shadow: none;
}
.cc-panel .cc-tile,
.cc-panel button.cc-button,
.cc-panel button.cc-edit {
    background: none;
    background-image: none;
    border: none;
    border-radius: 22px;
    box-shadow: none;
    outline: none;
    color: #ffffff;
    padding: 0;
    min-height: 0;
    min-width: 0;
}
.cc-panel button.cc-button:hover,
.cc-panel button.cc-button:active,
.cc-panel button.cc-edit:hover,
.cc-panel button.cc-edit:active {
    background: none;
    background-image: none;
    box-shadow: none;
}
.cc-panel button.cc-button:disabled {
    opacity: 0.5;
}
.cc-panel button.cc-circle,
.cc-panel button.cc-square {
    border-radius: 29px;
}
.cc-panel .cc-disc {
    border-radius: 17px;
    background-color: rgba(255, 255, 255, 0.24);
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.20), rgba(255, 255, 255, 0.04));
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.34), inset 0 1px 0 rgba(255, 255, 255, 0.40);
}
.cc-panel button.cc-pill {
    padding: 0 12px 0 12px;
}
.cc-panel button.cc-pill.on .cc-disc {
    background-color: #ffffff;
    background-image: none;
}
.cc-panel button.cc-pill.on .cc-disc .cc-icon {
    color: #0a67ff;
}
.cc-panel .cc-title {
    font-size: 13px;
    font-weight: 700;
    color: #ffffff;
    text-shadow: 0 0.5px 1.5px rgba(0, 30, 90, 0.30);
}
.cc-panel .cc-subtitle {
    font-size: 12px;
    font-weight: 500;
    color: #ffffff;
    opacity: 0.78;
}
.cc-panel .cc-art {
    border-radius: 12px;
    background-color: rgba(255, 255, 255, 0.26);
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.02));
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.30);
}
.cc-panel .cc-art picture {
    border-radius: 12px;
}
.cc-panel button.cc-transport {
    min-width: 30px;
    min-height: 30px;
    padding: 0;
    border: none;
    border-radius: 15px;
    background: transparent;
    background-image: none;
    box-shadow: none;
    color: #9be9ff;
}
.cc-panel button.cc-transport:hover {
    background-color: rgba(255, 255, 255, 0.20);
}
.cc-panel button.cc-transport:active {
    background-color: rgba(255, 255, 255, 0.30);
}
.cc-panel button.cc-transport image {
    color: #9be9ff;
}
.cc-panel button.cc-mini-circle {
    min-width: 26px;
    min-height: 26px;
    padding: 0;
    border: none;
    border-radius: 13px;
    background-color: rgba(255, 255, 255, 0.22);
    background-image: linear-gradient(to bottom, rgba(255, 255, 255, 0.16), rgba(255, 255, 255, 0.02));
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.32);
    color: #ffffff;
}
.cc-panel button.cc-mini-circle:hover {
    background-color: rgba(255, 255, 255, 0.32);
}
.cc-panel button.cc-mini-circle.on {
    background-color: rgba(255, 255, 255, 0.92);
    color: #0a67ff;
}
.cc-panel .cc-icon {
    color: #ffffff;
}
/* Sliders: a recessed translucent track, a bright fill, a white capsule knob. */
.cc-panel scale.cc-slider {
    min-height: 22px;
}
.cc-panel scale.cc-slider trough {
    min-height: 7px;
    border: none;
    border-radius: 4px;
    background-color: rgba(0, 24, 150, 0.34);
    box-shadow: inset 0 1px 2px rgba(0, 10, 90, 0.45), 0 0.5px 0 rgba(255, 255, 255, 0.16);
}
.cc-panel scale.cc-slider highlight {
    min-width: 0;
    min-height: 0;
    margin: 0;
    border: none;
    border-radius: 4px;
    background-color: rgba(255, 255, 255, 0.96);
    background-image: none;
    box-shadow: 0 0 6px rgba(255, 255, 255, 0.28);
}
.cc-panel scale.cc-slider slider {
    min-width: 32px;
    min-height: 20px;
    margin: -7px 0;
    border: none;
    border-radius: 10px;
    background-color: #ffffff;
    background-image: linear-gradient(to bottom, #ffffff, #eaf1fb);
    box-shadow: 0 2px 6px rgba(0, 20, 110, 0.38),
                0 0 0 0.5px rgba(0, 30, 120, 0.12),
                inset 0 1px 0 rgba(255, 255, 255, 1);
}
.cc-panel scale.cc-slider:hover slider {
    box-shadow: 0 3px 9px rgba(0, 20, 110, 0.44),
                0 0 0 0.5px rgba(0, 30, 120, 0.12),
                inset 0 1px 0 rgba(255, 255, 255, 1);
}
.cc-panel scale.cc-slider:disabled {
    opacity: 0.55;
}
.cc-panel button.cc-edit {
    padding: 3px 13px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
    color: #c9ebff;
}
";

pub fn install(display: &gdk::Display) -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();
    // The glass-coupled rules (shadow room, radii, popover accents, transitions)
    // come last so they sit on top of the static sheet.
    provider.load_from_string(&format!("{CSS}{}", crate::glass_config::css()));
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    provider
}
