<<<<<<< HEAD
# sonoma-desktop

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
- **Config location**, first found wins: `$SONOMA_DOCK_CONFIG`, `./config/dock.toml`, the
  project folder of the built binary, `~/.config/sonoma-desktop/dock.toml`, then the build
  folder. If none is usable, the bundled default is used.

## Requirements

- **KDE Plasma on Wayland**, with KWin's `org_kde_plasma_shell` protocol available.
  X11 and other Wayland compositors are not supported by this KDE-only build.
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
./target/release/sonoma-desktop          # run inside a KDE Plasma Wayland session
```
If the session does not provide Plasma shell surfaces, the program exits with an
explanatory message.

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
- **Bar height** is fixed at 28 logical px (`HEIGHT` in `src/menubar.rs`); tint and
  blur strength are constants in `src/glass.rs`.
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
src/glass.rs      the bar's frosted-glass widget (blurred strip + tint + hairline)
src/frost.rs      CPU crop + blur used by the glass
src/style.rs      the few lines of CSS that remove default window chrome
assets/           bundled wallpaper, dock-icons/ (your icons)
scripts/          wallpaper generator, nested-session launcher
```
=======
# Sonoma-desktop
>>>>>>> c254a1796cf7504c9dbcbf1e14beb0e739ddcfe0
