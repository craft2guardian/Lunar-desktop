//! Keeps one wallpaper + menu bar + dock set alive for every connected output,
//! following hot-plugging of displays.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio};

use crate::kwin_windows::KWinWindows;
use crate::{dock, menubar, style, wallpaper};

struct Output {
    monitor: gdk::Monitor,
    wallpaper: gtk::Window,
    menubar: gtk::Window,
    dock: gtk::Window,
}

pub struct Desktop {
    app: gtk::Application,
    image: RefCell<Rc<wallpaper::Image>>,
    /// Reloads the wallpaper when it is changed in Settings.
    wallpaper_observer: RefCell<Option<Rc<dyn Fn()>>>,
    /// Keep the display-level CSS provider alive for every shell widget.
    _style_provider: gtk::CssProvider,
    kwin_windows: Option<KWinWindows>,
    outputs: RefCell<Vec<Output>>,
    /// Keeps the application alive even with zero outputs connected.
    _hold: gio::ApplicationHoldGuard,
}

impl Desktop {
    /// Loads the wallpaper, creates surfaces for the current outputs and keeps
    /// them in sync from here on. Returns an error message if it cannot start.
    pub fn start(app: &gtk::Application) -> Result<(), String> {
        let display = gdk::Display::default().ok_or("no display available")?;
        let image = wallpaper::Image::load_current()
            .map_err(|e| format!("could not decode the wallpaper: {e}"))?;
        let style_provider = style::install(&display);
        let kwin_windows = match KWinWindows::start() {
            Ok(tracker) => Some(tracker),
            Err(error) => {
                eprintln!("[dock-track-debug][tracker-unavailable] {error}");
                None
            }
        };

        let desktop = Rc::new(Desktop {
            app: app.clone(),
            image: RefCell::new(Rc::new(image)),
            wallpaper_observer: RefCell::new(None),
            _style_provider: style_provider,
            kwin_windows,
            outputs: RefCell::new(Vec::new()),
            _hold: app.hold(),
        });

        desktop.sync(&display);

        let observer: Rc<dyn Fn()> = {
            let weak = Rc::downgrade(&desktop);
            Rc::new(move || {
                if let Some(desktop) = weak.upgrade() {
                    desktop.reload_wallpaper();
                }
            })
        };
        crate::preferences::observe_wallpaper(&observer);
        *desktop.wallpaper_observer.borrow_mut() = Some(observer);

        // The monitor list changes when a display is plugged in or removed.
        let watcher = Rc::clone(&desktop);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(display) = gdk::Display::default() {
                watcher.sync(&display);
            }
        });
        Ok(())
    }

    /// Text colour for the menu bar of `monitor`, from the wallpaper under it.
    fn bar_tone(image: &wallpaper::Image, monitor: &gdk::Monitor) -> wallpaper::BarTone {
        let geometry = monitor.geometry();
        image.bar_tone(
            (geometry.width() as f32, geometry.height() as f32),
            menubar::HEIGHT as f32,
        )
    }

    /// Text colour and blurred-wallpaper backdrop of the menu bar of `monitor`.
    /// `force` rebuilds the backdrop even if the output is unchanged.
    fn style_bar(window: &gtk::Window, image: &wallpaper::Image, monitor: &gdk::Monitor, force: bool) {
        menubar::set_tone(window, Self::bar_tone(image, monitor));
        menubar::set_backdrop(window, image, monitor, force);
    }

    /// Shows the wallpaper currently chosen in Settings on every output.
    fn reload_wallpaper(&self) {
        match wallpaper::Image::load_current() {
            Ok(image) => {
                let image = Rc::new(image);
                for output in self.outputs.borrow().iter() {
                    wallpaper::set_image(&output.wallpaper, &image);
                    Self::style_bar(&output.menubar, &image, &output.monitor, true);
                }
                *self.image.borrow_mut() = image;
            }
            Err(error) => eprintln!("lunar-desktop: could not load the wallpaper: {error}"),
        }
    }

    /// Reconciles the open surfaces with the monitors that currently exist.
    fn sync(&self, display: &gdk::Display) {
        let current: Vec<gdk::Monitor> = display
            .monitors()
            .iter::<gdk::Monitor>()
            .filter_map(Result::ok)
            .filter(|m| m.is_valid())
            .collect();

        let mut outputs = self.outputs.borrow_mut();

        outputs.retain(|output| {
            let still_connected = current.contains(&output.monitor);
            if !still_connected {
                output.dock.destroy();
                output.menubar.destroy();
                output.wallpaper.destroy();
            }
            still_connected
        });

        for monitor in current {
            if outputs.iter().any(|o| o.monitor == monitor) {
                continue;
            }
            let image = self.image.borrow().clone();
            let wallpaper = wallpaper::window(&self.app, &image, &monitor);
            let menubar = menubar::window(&self.app, &monitor, self.kwin_windows.clone());
            let dock = dock::window(&self.app, &monitor, self.kwin_windows.clone());
            Self::style_bar(&menubar, &image, &monitor, false);
            // Background first, so the bar always stacks above it.
            wallpaper.present();
            menubar.present();
            dock.present();
            outputs.push(Output {
                monitor,
                wallpaper,
                menubar,
                dock,
            });
        }

        // A changed resolution or aspect ratio changes what the bar sits on.
        let image = self.image.borrow().clone();
        for output in outputs.iter() {
            Self::style_bar(&output.menubar, &image, &output.monitor, false);
        }
    }
}
