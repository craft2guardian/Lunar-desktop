//! The menu bar's background. It asks KWin for no effect (so the glass plugin
//! never touches it); instead it paints a blurred copy of the wallpaper behind
//! it (see `frost.rs`, set with `set_backdrop`), a light wash over that, and
//! the bottom hairline. The wash and hairline values live in `glass_config.rs`
//! (`STRIP`), so they follow the global glass tuning.

use std::cell::{Cell, RefCell};

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
    pub struct GlassStrip {
        /// Blurred wallpaper strip, drawn at `scale` device pixels per pixel.
        pub backdrop: RefCell<Option<gdk::Texture>>,
        pub scale: Cell<i32>,
        /// (output width, output height, scale) the backdrop was built for.
        pub key: Cell<Option<(i32, i32, i32)>>,
    }

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
            // Blurred wallpaper first, cropped to the bar (the texture also
            // holds blur context below the bar).
            if let Some(texture) = self.backdrop.borrow().as_ref() {
                let k = self.scale.get().max(1) as f32;
                snapshot.push_clip(&bounds);
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(0.0, 0.0, texture.width() as f32 / k, texture.height() as f32 / k),
                );
                snapshot.pop();
            }
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

    /// What the current backdrop was built for, if there is one.
    pub fn backdrop_key(&self) -> Option<(i32, i32, i32)> {
        self.imp().key.get()
    }

    /// Sets the blurred wallpaper strip. `scale` is its device pixels per
    /// logical pixel; `key` identifies the output it was built for.
    pub fn set_backdrop(&self, texture: gdk::Texture, scale: i32, key: (i32, i32, i32)) {
        let imp = self.imp();
        imp.backdrop.replace(Some(texture));
        imp.scale.set(scale);
        imp.key.set(Some(key));
        self.queue_draw();
    }
}
