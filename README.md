# lunar-desktop

A minimal macOS Sonoma-inspired desktop shell for Linux, written in Rust.

**Milestone 2** shows three things on every screen (milestone 1 was the first two):

1. a full-screen, edge-to-edge landscape wallpaper, and
2. a thin (28 px), full-width, empty, frosted-glass menu bar flush with the top edge.

3. a centred, rounded, translucent **Dock** near the bottom. Clicking launches an app;
   a small dot marks running apps, and Finder stays marked open. Entries come from
   `config/dock.toml` (see below).
   The Dock is a floating panel surface styled to match the menu bar.

Nothing else: no logo, text, clock, menus, widgets or desktop icons.

## How it works (and what "desktop" means here)

A normal application window is **not** a desktop background or a system menu bar
on Linux. It gets a title bar, can be moved, stacked and covered, and does not
reserve screen space. Implementing a real compositor/window manager would be far
out of scope, so this project uses KWin's Plasma shell surface protocol on KDE
Plasma Wayland:

| Surface   | Plasma role | Position                         | Notes                                  |
|-----------|-------------|----------------------------------|----------------------------------------|
| Wallpaper | `desktop`   | output origin                    | One full-size window per display       |
| Menu bar  | `panel`     | top edge                         | Full output width, 28 px tall          |
| Dock      | `panel`     | centred at the bottom of output  | Floating panel with the existing icons |

The windows have no decorations, are excluded from the taskbar and switcher,
and KWin handles their stacking and output placement.

Other design points:

- **Cover scaling.** The wallpaper uses `gtk::Picture` with `ContentFit::Cover`:
  aspect ratio preserved, centred, overflow cropped. It is a static GPU texture,
  so nothing redraws while idle.
- **Frosted glass.** Wayland has no portable "blur what is behind me", but the bar
  only ever sits directly on the wallpaper, so the bar blurs *the wallpaper
  itself*: it crops it with exactly the same cover maths as the wallpaper window,
  blurs it on the CPU with edge clamping (so the ends don't fade or shift), adds a
  12 % white tint and a 1 px hairline. This is computed once per output size or
  scale change, never per frame. The result lines up with the wallpaper below it.
- **Multiple / changing displays.** One wallpaper + bar pair is created per
  monitor and follows hot-plug and unplug. Resolution and scale changes are
  handled by the compositor reconfiguring the surfaces; the glass re-renders
  itself when the geometry or scale factor changes.
- **Dock indicators.** The Dock checks running app processes once per second.
  Finder always shows as open.

## Customising the Dock

Edit `config/dock.toml` and restart the shell. Each `[[app]]` block is one icon, shown in
file order:

```toml
[[app]]
name = "Browser"                         # tooltip text
icon = "assets/dock-icons/browser.png"   # optional PNG
command = "firefox --new-window"         # program + arguments
```

- **Add an icon:** drop a square PNG (128 px or larger, transparent background) into
  `assets/dock-icons/` and set `icon` to its path. Relative paths are looked up next to the
  config file and in the project folder; `~/` and absolute paths also work. Replacing the
  image file is enough to change the icon. No icons are bundled.
- **Fallbacks:** a missing or unreadable icon falls back to the installed application's own
  icon, then to a neutral lettered tile. Problems are printed to stderr.
- **Commands are not run through a shell.** They are split into a program and arguments
  (quotes group words), so pipes, `$VARS` and `~` are not interpreted. A command that fails
  to start is logged and otherwise ignored.
- **Config location**, first found wins: `$LUNAR_DOCK_CONFIG`, `./config/dock.toml`, the
  project folder of the built binary, `~/.config/lunar-desktop/dock.toml`, then the build
  folder. If none is usable, the bundled default is used.

## Tuning the Liquid Glass

All glass appearance is configured in `src/glass_config.rs`; no component hardcodes a
colour, opacity, radius, shadow, highlight or timing.

- **Global knobs** (`TUNING`): opacity, tint saturation, blur radius, frost, refraction,
  highlight, reflection, border opacity/width, edge light, shadow intensity/softness/spread,
  corner radius, depth, light angle, animation speed and easing. `1.0` means "as designed",
  so list only what you change:

  ```rust
  pub const TUNING: Tuning = Tuning { opacity: 0.85, refraction: 1.4, ..NEUTRAL };
  ```
- **Per-material presets** (`presets::CLEAR`, `ACCENT`, `MENU`, `DOCK`, `BUBBLE`, `SIDEBAR`):
  each has the same sections (body, blur, refraction, highlight, reflection, border, edge,
  shadow, depth, motion). Add a material by adding a preset and a `Preset::name()` constructor.
- **Shared behaviour**: `INTERACTION` (hover/press response), `DARK` (dark-mode adaptation),
  `MENU_CHROME` (popover arrow, menu-item hover capsule, open-menu capsule), `STRIP` (menu bar).
- **Kept in sync automatically**: the compositor blur regions take their corner radius from the
  material, and the CSS that must agree with the glass (shadow room, radii, popover accents,
  transitions) is generated from the config by `glass_config::css()`.

`LUNAR_GLASS_PREVIEW=all cargo run` renders the surfaces off-compositor for judging changes.

## Requirements

- **KDE Plasma on Wayland**, with KWin's `org_kde_plasma_shell` protocol available.
  X11 is not supported.
- Rust (tested with 1.97)
- GTK 4.12 or newer, with development headers
- Wayland client development headers
- `pkg-config`, a C toolchain

### Installing the dependencies

Ubuntu / Debian:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libwayland-dev
```

## Build and run

```sh
cargo build --release
./target/release/lunar-desktop          # run inside a KDE Plasma Wayland session
```
If the session does not provide Plasma shell surfaces, the program exits with an
explanatory message.

## Graphical installer

`installer/lunar-installer.py` is a one-file GTK 4 installer (needs only Python 3
with PyGObject, which Fedora KDE ships) that works on top of any desktop. It
installs the build tools, downloads and builds Lunar from GitHub, optionally
builds the KWin glass effect and installs the WhiteSur icon theme, then adds the
Lunar entry to the login screen. It asks for your password twice (`pkexec`) and
saves its log to `~/.cache/lunar-installer/install.log`.

    python3 installer/lunar-installer.py            # install (Fedora, dnf)
    python3 installer/lunar-installer.py --demo     # click through, changes nothing

`installer/Install-Lunar.sh` is the same installer as one self-contained file for
other people: it checks that GTK 4 is available (offering to install it on
Fedora), shows problems in dialog boxes, and runs by double-click once marked
executable. Rebuild it after editing the installer with
`sh installer/make-single-file.sh`.

The repositories it clones can be overridden with `LUNAR_REPO`,
`LUNAR_GLASS_REPO` and `LUNAR_ICONS_REPO`.

## Limitations of this first implementation

- **It is not a window manager or a system-wide menu bar.** It creates three KDE
  Plasma surfaces through KWin. There is no global menu, no
  integration with other apps, and no session management. Disable the Plasma
  desktop wallpaper or other panels if you do not want them to coexist.
- **KDE Plasma Wayland only.** The shell uses KWin's Plasma shell protocol for
  desktop and panel placement; that protocol is a KDE implementation detail.
- **Blur support.** The menu popover requests KWin's Wayland background effect.
  The menu remains translucent if the compositor does not advertise blur.
- **Bundled wallpaper only.** It is original procedurally generated artwork in a
  Sonoma-like style (not Apple's image). There is no runtime configuration yet: to
  change it, replace `assets/wallpaper.jpg` (or edit and run
  `python3 scripts/generate_wallpaper.py`, which needs numpy and Pillow) and rebuild.
  The image is decoded once and shared by all outputs.
- **Bar height** is fixed at 28 logical px (`HEIGHT` in `src/menubar.rs`); the bar's
  tint and hairline are `STRIP` in `src/glass_config.rs`. KWin's blur strength is a
  global compositor setting that an app cannot change.
- **Fractional scaling** is rendered at the next integer scale and downsampled by
  GTK, so the glass is slightly oversampled at e.g. 1.5x.
- **Runtime status:** buildable, but visual behavior depends on the active KDE
  Plasma Wayland session and has not been verified in a real display session.

## Development

```sh
cargo test        # cover-crop maths and blur unit tests
```

### Layout

```
src/main.rs       entry point, Plasma protocol support check
src/kde_shell.rs  KDE Wayland surface integration
src/kde_shell.c   Plasma shell protocol bindings and panel positioning
src/desktop.rs    one wallpaper + bar per monitor, hot-plug handling
src/wallpaper.rs  wallpaper window, image loading, cover-crop geometry
src/dock.rs       dock window, icon fallbacks, launching
src/config.rs     reads config/dock.toml
config/           dock.toml (Dock entries)
src/menubar.rs    menu bar window and KDE menu
src/glass_config.rs  every Liquid Glass value: tuning knobs, material presets, motion
src/liquid_glass.rs  the glass painter + GlassSurface widget (reads glass_config only)
src/glass.rs      the bar's frosted-glass widget (tint + hairline, from glass_config)
src/frost.rs      CPU crop + blur used by the glass
src/style.rs      the few lines of CSS that remove default window chrome
assets/           bundled wallpaper, dock-icons/ (your icons)
scripts/          wallpaper generator, nested-session launcher
```


## Minimised-window thumbnails

With **Settings → Desktop & Dock → Minimize windows into application icon** off,
each minimised window gets its own tile between the Dock separator and the Trash,
showing a thumbnail of the window with the app icon as a badge. Thumbnails come
from KWin's screenshot interface, which only answers apps that declare it in
their desktop file. Run `scripts/install-thumbnail-permission.sh` once (pass the
path of the binary you run) and restart the shell. Without it the tile shows a
placeholder card instead. KWin cannot capture a window once it is minimised, so
the Dock refreshes a thumbnail of each visible window about every 1.5 seconds
while the switch is off and shows the latest one.


## The Lunar session (KWin + Lunar, no plasmashell)

Lunar can run as a complete desktop of its own: KWin is the window manager and
compositor, Lunar is the shell (wallpaper, menu bar, dock), and Plasma's own
shell never starts. It installs as a separate entry at the login screen, so
your normal Plasma session is left alone.

    cargo build --release
    sudo session/install.sh              # add --prefix /usr to install under /usr
    # log out, then pick "Lunar" in the session menu on the login screen

To go back, pick "Plasma" at the login screen. To remove it again:
`sudo session/install.sh --uninstall`. The session log is
`~/.local/state/lunar-desktop/session.log`.

What the session starts, besides KWin and Lunar: KDE's global-shortcut service
and settings daemon, the polkit password agent, and a notification daemon
(`mako`, `dunst` or `swaync`, whichever is installed; Plasma's own notification
popups lived in plasmashell, so install one of them). Needs Plasma 6 (KWin and
`kglobalacceld`) and a systemd user session.

This session is new and has had little testing. If something does not start,
the log says which helper was skipped.


## Liquid Glass

Surfaces use a shared material system in `src/liquid_glass.rs`. KWin still
supplies the backdrop-dependent blur (`ext-background-effect-v1`); everything
that makes the result read as glass is painted by `GlassSurface`:

- a luminance-graded body fill (and an accent tint that crossfades when a
  control's `.on` class is set),
- an inner meniscus glow that stands in for edge refraction,
- directional specular lines (key light upper left, weak bounce lower right),
- a diagonal-gradient rim stroke, a sheen over the upper part, and two-stage
  shadows,
- pointer response on interactive surfaces: lift on hover, compression on
  press, a glint that follows the cursor, and accent glass that thins to clear
  glass on hover. These run on the frame clock, so they work although
  `animation.rs` keeps GTK's own animations off.

Each treatment is a `Material` preset: `clear()` (Control Centre tiles),
`accent()` (sliders and shortcut buttons), `menu()` (menus and popovers),
`dock()` (the shelf), `bubble()` (Dock labels) and `sidebar()`. Use
`GlassSurface::wrap(&widget, Material::...)` to put any widget on glass, or
`liquid_glass::glass_popover(&popover, Material::menu())` for a popover body.
Layout and colours of the content stay in `style.rs`.

### Previewing without KWin

```sh
GDK_BACKEND=x11 GSK_RENDERER=cairo LUNAR_GLASS_PREVIEW=all \
LUNAR_GLASS_WALLPAPER=assets/glass-preview-wallpaper.jpg LUNAR_GLASS_OUT=out.png \
  xvfb-run -a ./target/debug/lunar-desktop
```

Scenes: `cc`, `menu`, `dock`, `all`. Add `LUNAR_GLASS_DARK=1` for Dark Mode and
`LUNAR_GLASS_HOVER=1` for the hover/glint state. The preview blurs the
wallpaper itself where KWin would blur the backdrop (see `glass_preview.rs`).
