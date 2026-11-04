//! Translucent tint for the menu bar. KWin supplies the actual backdrop blur
//! through ext-background-effect-v1; this widget only paints a light wash and
//! the bottom hairline.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene};

/// Wash used when the bar text is dark (a bright wallpaper behind the bar).
pub const LIGHT_TINT_ALPHA: f32 = 0.30;
/// Wash used when the bar text is light (a dark wallpaper behind the bar).
pub const DARK_TINT_ALPHA: f32 = 0.45;
const LIGHT_TINT: gdk::RGBA = gdk::RGBA::new(1.0, 1.0, 1.0, LIGHT_TINT_ALPHA);
const DARK_TINT: gdk::RGBA = gdk::RGBA::new(0.27, 0.29, 0.32, DARK_TINT_ALPHA);
const HAIRLINE: gdk::RGBA = gdk::RGBA::new(0.0, 0.0, 0.0, 0.10);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GlassStrip;

    #[glib::object_subclass]
    impl ObjectSubclass for GlassStrip {
        const NAME: &'static str = "LunarGlassStrip";
        type Type = super::GlassStrip;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for GlassStrip {}

    impl WidgetImpl for GlassStrip {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width(), widget.height());
            if width <= 0 || height <= 0 {
                return;
            }

            let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            // The tint follows what is behind the bar (see `wallpaper::bar_tone`),
            // not the dark-mode setting: the window carries the class.
            let light_text = widget
                .root()
                .is_some_and(|root| root.upcast_ref::<gtk::Widget>().has_css_class("bar-light-text"));
            let tint = if light_text {
                &DARK_TINT
            } else {
                &LIGHT_TINT
            };
            snapshot.append_color(tint, &bounds);
            snapshot.append_color(
                &HAIRLINE,
                &graphene::Rect::new(0.0, (height - 1) as f32, width as f32, 1.0),
            );
        }
    }
}

glib::wrapper! {
    pub struct GlassStrip(ObjectSubclass<imp::GlassStrip>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GlassStrip {
    pub fn new() -> Self {
        glib::Object::new()
    }
}
