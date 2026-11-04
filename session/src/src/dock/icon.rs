//! Custom rounded dock icon widget and icon loading helpers.

use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use crate::config::Entry;
use crate::kwin_windows;

pub(crate) const ICON_CORNER_RADIUS_FACTOR: f32 = 0.22;

/// Launch bounce: seconds per hop, and the tallest hop in px. The Dock window
/// has 24px of transparent margin above the shelf (see `.dock` in style.rs), so
/// the icon can rise this far without being clipped.
pub(crate) const BOUNCE_PERIOD_SECONDS: f64 = 0.75;
pub(crate) const BOUNCE_MAX_LIFT: f32 = 11.3;

mod rounded_icon_imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct RoundedIcon {
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) size: std::cell::Cell<i32>,
        /// Current launch-bounce height in px (0 when at rest).
        pub(super) lift: std::cell::Cell<f32>,
        pub(super) bouncing: std::cell::Cell<bool>,
        pub(super) stop_bounce: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoundedIcon {
        const NAME: &'static str = "LunarRoundedDockIcon";
        type Type = super::RoundedIcon;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for RoundedIcon {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for RoundedIcon {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.size.get().max(0);
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let child = self.child.borrow();
            let Some(child) = child.as_ref() else {
                return;
            };
            let widget = self.obj();
            let (width, height) = (widget.width(), widget.height());
            if width <= 0 || height <= 0 {
                return;
            }
            let lift = self.lift.get();
            if lift != 0.0 {
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(0.0, -lift));
            }
            let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            let radius = width.min(height) as f32 * ICON_CORNER_RADIUS_FACTOR;
            let clip = gtk::gsk::RoundedRect::from_rect(bounds, radius);
            snapshot.push_rounded_clip(&clip);
            widget.snapshot_child(child, snapshot);
            snapshot.pop();
            if lift != 0.0 {
                snapshot.restore();
            }
        }
    }
}

glib::wrapper! {
    pub struct RoundedIcon(ObjectSubclass<rounded_icon_imp::RoundedIcon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl RoundedIcon {
    pub(crate) fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let icon: Self = glib::Object::new();
        child.set_parent(&icon);
        icon.imp().child.replace(Some(child.clone().upcast()));
        icon
    }

    pub(crate) fn set_icon_size(&self, size: i32) {
        self.imp().size.set(size.max(0));
        self.queue_resize();
    }

    /// Hops the icon up and down like a macOS Dock icon while its app opens.
    /// Keeps hopping until `stop_bounce` is called or `max` has elapsed, and
    /// always finishes the hop in progress so the icon lands at rest.
    /// `on_done` runs once it has landed.
    pub(crate) fn start_bounce(&self, max: Duration, on_done: impl Fn() + 'static) {
        let imp = self.imp();
        imp.stop_bounce.set(false);
        if imp.bouncing.replace(true) {
            return;
        }
        let began = std::cell::Cell::new(None::<i64>);
        let last_phase = std::cell::Cell::new(0.0_f64);
        self.add_tick_callback(move |icon, clock| {
            let imp = icon.imp();
            let now = clock.frame_time();
            let start = match began.get() {
                Some(start) => start,
                None => {
                    began.set(Some(now));
                    now
                }
            };
            let elapsed = (now - start) as f64 / 1_000_000.0;
            let phase = (elapsed / BOUNCE_PERIOD_SECONDS).fract();
            let wrapped = phase < last_phase.replace(phase);
            if wrapped && (imp.stop_bounce.get() || elapsed >= max.as_secs_f64()) {
                imp.lift.set(0.0);
                imp.bouncing.set(false);
                icon.queue_draw();
                on_done();
                return glib::ControlFlow::Break;
            }
            // A parabola: quick off the ground, slow at the top, like gravity.
            let peak = (icon.height() as f32 * 0.5).min(BOUNCE_MAX_LIFT);
            let p = phase as f32;
            imp.lift.set(peak * 4.0 * p * (1.0 - p));
            icon.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    /// Lets the current hop finish, then stops bouncing.
    pub(crate) fn stop_bounce(&self) {
        self.imp().stop_bounce.set(true);
    }
}

#[derive(Clone)]
pub(crate) struct DockIcon {
    pub(crate) picture: gtk::Picture,
}

pub(crate) fn icon_for(entry: &Entry, display: &gdk::Display) -> Option<DockIcon> {
    if let Some(path) = &entry.icon {
        match gdk::Texture::from_filename(path) {
            Ok(texture) => {
                return Some(DockIcon {
                    picture: picture_for(&texture),
                });
            }
            Err(e) => eprintln!(
                "lunar-desktop: icon {} unavailable for \"{}\" ({e}); using fallback",
                path.display(),
                entry.name
            ),
        }
    }

    let program = super::launcher::program_name(&entry.command)?;
    let theme = gtk::IconTheme::for_display(display);
    if entry.command == kwin_windows::SETTINGS_IDENTITY {
        let name = ["preferences-system", "systemsettings", "org.kde.systemsettings"]
            .into_iter()
            .find(|name| theme.has_icon(name))?;
        let paintable = theme.lookup_icon(
            name,
            &[],
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        return Some(DockIcon {
            picture: picture_for(&paintable),
        });
    }
    if let Some(icon) = super::launcher::installed_app_for(&program).and_then(|app| app.icon()) {
        let paintable = theme.lookup_by_gicon(
            &icon,
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        return Some(DockIcon {
            picture: picture_for(&paintable),
        });
    }
    theme.has_icon(&program).then(|| {
        let paintable = theme.lookup_icon(
            &program,
            &[],
            64,
            1,
            gtk::TextDirection::None,
            gtk::IconLookupFlags::empty(),
        );
        DockIcon {
            picture: picture_for(&paintable),
        }
    })
}

pub(crate) fn picture_for(paintable: &impl IsA<gdk::Paintable>) -> gtk::Picture {
    let picture = gtk::Picture::for_paintable(paintable);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_size_request(36, 36);
    picture
}
