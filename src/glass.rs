//! Translucent tint for the menu bar. KWin supplies the actual backdrop blur
//! through ext-background-effect-v1; this widget only paints a light wash and
//! the bottom hairline. The wash and hairline values live in `glass_config.rs`
//! (`STRIP`), so they follow the global glass tuning.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene};

fn to_rgba(c: [f32; 4]) -> gdk::RGBA {
    gdk::RGBA::new(c[0], c[1], c[2], c[3])
}

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
            let strip = crate::glass_config::strip();
            let tint = if light_text { strip.dark_tint } else { strip.light_tint };
            snapshot.append_color(&to_rgba(tint), &bounds);
            let line = strip.hairline_px;
            snapshot.append_color(
                &to_rgba(strip.hairline),
                &graphene::Rect::new(0.0, height as f32 - line, width as f32, line),
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
