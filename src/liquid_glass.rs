//! Liquid Glass: the shell's material system.
//!
//! The compositor (KWin, through `ext-background-effect-v1`) supplies the
//! backdrop-dependent blur. Everything that makes the result read as *glass*
//! rather than "frosted rectangle" is painted here, in one place, so every
//! surface (Control Centre tiles, menus, the Dock, ...) shares the same
//! optical model and differs only in its [`Material`]:
//!
//! * a body fill with a vertical luminance gradient (lighter at the top, like
//!   light entering a curved slab) and an optional accent tint,
//! * an inner "meniscus" glow that hugs the edge, which is how refraction at
//!   the rim of a thick lens reads when the backdrop itself cannot be warped,
//! * directional specular lines (light from the upper left, a weaker bounce
//!   from the lower right) drawn as inset shadows,
//! * a one-device-pixel-class rim stroke whose brightness follows a diagonal
//!   gradient (bright, dim, bright) instead of a flat border colour,
//! * a soft sheen over the upper part of the surface,
//! * two-stage drop shadows (ambient + contact).
//!
//! Interactive surfaces also respond to the pointer: they lift slightly on
//! hover, compress on press, and a glint follows the cursor across the glass.
//! These animations run on the frame clock and therefore keep working while
//! GTK's own animations are switched off (see `animation.rs`).
//!
//! No visual value is defined in this file: materials, optics, shadows, motion
//! and interaction response all come from `glass_config.rs`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene, gsk};

use crate::glass_config::{self, mix, Easing, Rgba};

// ---------------------------------------------------------------- materials
//
// Every visual value lives in `glass_config.rs`. A `Material` is one preset
// from there (`Material::menu()`, `Material::dock()`, ...); the global tuning
// is applied each time a surface is painted, so a changed value reaches every
// surface without touching this file.
pub use crate::glass_config::{Material, Radius};

fn gdk_rgba(c: Rgba, alpha_scale: f32) -> gdk::RGBA {
    gdk::RGBA::new(c[0], c[1], c[2], (c[3] * alpha_scale).clamp(0.0, 1.0))
}

// ------------------------------------------------------------ preview hook

/// Draws the (already blurred) backdrop for a glass surface. The real shell
/// never sets this: KWin blurs behind the window. The preview harness uses it
/// to stand in for that blur so the glass can be judged off-compositor. The
/// last argument is the material's blur radius in logical pixels.
pub type Backdrop = Rc<dyn Fn(&gtk::Snapshot, &graphene::Rect, f32)>;

thread_local! {
    static BACKDROP: RefCell<Option<Backdrop>> = const { RefCell::new(None) };
}

#[allow(dead_code)]
pub fn set_preview_backdrop(backdrop: Option<Backdrop>) {
    BACKDROP.with(|slot| *slot.borrow_mut() = backdrop);
}

// ------------------------------------------------------------------ painter

/// Animated, per-surface interaction state.
#[derive(Clone, Copy, Default)]
struct Look {
    hover: f32,
    press: f32,
    on: f32,
    /// Pointer position relative to the surface, 0..1.
    px: f32,
    py: f32,
}

fn resolve_radius(radius: Radius, w: f32, h: f32) -> f32 {
    let limit = (w.min(h) / 2.0).max(0.0);
    match radius {
        Radius::Fixed(r) => r.min(limit),
        Radius::Capsule => limit,
    }
}

fn outline(w: f32, h: f32, r: f32) -> gsk::RoundedRect {
    gsk::RoundedRect::new(
        graphene::Rect::new(0.0, 0.0, w, h),
        graphene::Size::new(r, r),
        graphene::Size::new(r, r),
        graphene::Size::new(r, r),
        graphene::Size::new(r, r),
    )
}

fn stops(list: &[(f32, gdk::RGBA)]) -> Vec<gsk::ColorStop> {
    list.iter().map(|(o, c)| gsk::ColorStop::new(*o, *c)).collect()
}

/// Everything beneath the content: shadow, backdrop stand-in, body, glow.
fn paint_under(snapshot: &gtk::Snapshot, bounds: &graphene::Rect, m: &Material, look: Look, origin: &graphene::Rect) {
    let (w, h) = (bounds.width(), bounds.height());
    let r = resolve_radius(m.radius, w, h);
    let shape = outline(w, h, r);
    let ix = glass_config::INTERACTION;
    let lift = 1.0 + ix.hover_boost * look.hover;

    // Outer shadows: a broad ambient one and a tight contact one.
    if m.shadow.intensity > 0.0 {
        let c = m.shadow.color;
        for layer in [m.shadow.ambient, m.shadow.contact] {
            let alpha = (layer.alpha * m.shadow.intensity * lift).clamp(0.0, 1.0);
            snapshot.append_outset_shadow(
                &shape,
                &gdk::RGBA::new(c[0], c[1], c[2], alpha),
                layer.dx,
                layer.dy,
                layer.spread,
                layer.blur,
            );
        }
    }

    snapshot.push_rounded_clip(&shape);

    // Preview-only stand-in for the compositor's backdrop blur.
    BACKDROP.with(|slot| {
        if let Some(backdrop) = slot.borrow().as_ref() {
            backdrop(snapshot, origin, m.blur.radius);
        }
    });
    // Frost haze over the backdrop: the app-side way to read as more diffuse.
    if m.blur.frost[3] > 0.0 {
        snapshot.append_color(&gdk_rgba(m.blur.frost, 1.0), bounds);
    }

    // Body fill. Switching on crossfades to the "on" fill.
    let mut top = mix(m.body.top, m.body.on_top, look.on);
    let mut bottom = mix(m.body.bottom, m.body.on_bottom, look.on);
    if m.hover_clears {
        // Thin to a neutral, lightly frosted body so the backdrop lenses through.
        let amount = look.hover * m.body.hover_clear_amount;
        top = mix(top, m.body.hover_clear_top, amount);
        bottom = mix(bottom, m.body.hover_clear_bottom, amount);
    }
    let hover_lift = ix.fill_hover_lift * look.hover;
    let press_dim = ix.press_dim * look.press;
    snapshot.append_linear_gradient(
        bounds,
        &graphene::Point::new(0.0, 0.0),
        &graphene::Point::new(0.0, h),
        &stops(&[
            (0.0, gdk_rgba(top, 1.0 + hover_lift * 2.0)),
            (1.0, gdk_rgba(bottom, 1.0 + hover_lift * 2.0)),
        ]),
    );
    if hover_lift > 0.0 {
        snapshot.append_color(&gdk::RGBA::new(1.0, 1.0, 1.0, hover_lift), bounds);
    }
    if press_dim > 0.0 {
        let c = ix.press_dim_color;
        snapshot.append_color(&gdk::RGBA::new(c[0], c[1], c[2], press_dim), bounds);
    }

    // Meniscus: the luminous band at the edge where a thick lens bends the
    // backdrop most. Two inset glows (wide + tight) read as a graded edge.
    let rf = &m.refraction;
    let reach = (r * rf.reach).clamp(rf.reach_min, rf.reach_max);
    snapshot.append_inset_shadow(
        &shape,
        &gdk_rgba(rf.glow, 1.0 + ix.glow_hover_boost * look.hover),
        0.0,
        0.0,
        0.0,
        reach,
    );
    snapshot.append_inset_shadow(
        &shape,
        &gdk_rgba(rf.glow, rf.tight_alpha),
        0.0,
        0.0,
        0.0,
        (reach * rf.tight_reach).max(rf.tight_min),
    );

    snapshot.pop();
}

/// Offset that pushes a highlight line in from the edge facing a light that
/// comes from `azimuth` degrees (clockwise from the top), `distance` px deep.
fn light_offset(azimuth_deg: f32, distance: f32) -> (f32, f32) {
    let a = azimuth_deg.to_radians();
    (-a.sin() * distance, a.cos() * distance)
}

/// Everything above the content: specular lines, sheen, glint, rim.
fn paint_over(snapshot: &gtk::Snapshot, bounds: &graphene::Rect, m: &Material, look: Look) {
    let (w, h) = (bounds.width(), bounds.height());
    let r = resolve_radius(m.radius, w, h);
    let shape = outline(w, h, r);
    let ix = glass_config::INTERACTION;
    let boost = 1.0 + ix.hover_boost * look.hover;

    snapshot.push_rounded_clip(&shape);

    // Broad sheen over the upper part: a curved slab catches the sky.
    let rf = &m.reflection;
    if rf.strength > 0.0 {
        let sheen_h = (h * rf.coverage).max(1.0);
        let k = rf.strength * boost;
        snapshot.append_linear_gradient(
            &graphene::Rect::new(0.0, 0.0, w, sheen_h),
            &graphene::Point::new(0.0, 0.0),
            &graphene::Point::new(0.0, sheen_h),
            &stops(&[
                (0.0, gdk::RGBA::new(1.0, 1.0, 1.0, (rf.top_alpha * k).clamp(0.0, 1.0))),
                (rf.mid_stop, gdk::RGBA::new(1.0, 1.0, 1.0, (rf.mid_alpha * k).clamp(0.0, 1.0))),
                (1.0, gdk::RGBA::new(1.0, 1.0, 1.0, 0.0)),
            ]),
        );
    }

    // Directional specular lines: a key light and a weak bounce opposite it.
    let hl = &m.highlight;
    if hl.strength > 0.0 {
        let s = hl.strength * boost;
        let (kx, ky) = light_offset(hl.azimuth, hl.key_distance);
        snapshot.append_inset_shadow(
            &shape,
            &gdk::RGBA::new(1.0, 1.0, 1.0, (hl.key_alpha * s).clamp(0.0, 1.0)),
            kx,
            ky,
            0.0,
            hl.key_softness,
        );
        let (bx, by) = light_offset(hl.azimuth + 180.0, hl.bounce_distance);
        snapshot.append_inset_shadow(
            &shape,
            &gdk::RGBA::new(1.0, 1.0, 1.0, (hl.bounce_alpha * s).clamp(0.0, 1.0)),
            bx,
            by,
            0.0,
            hl.bounce_softness,
        );
    }

    // Cursor glint: a soft radial highlight following the pointer.
    if look.hover > 0.01 {
        let cx = look.px * w;
        let cy = look.py * h;
        let radius = (w.max(h) * hl.glint_radius).max(hl.glint_min_radius);
        snapshot.append_radial_gradient(
            bounds,
            &graphene::Point::new(cx, cy),
            radius,
            radius,
            0.0,
            1.0,
            &stops(&[
                (0.0, gdk::RGBA::new(1.0, 1.0, 1.0, (hl.glint_alpha * look.hover).clamp(0.0, 1.0))),
                (0.45, gdk::RGBA::new(1.0, 1.0, 1.0, (hl.glint_falloff_alpha * look.hover).clamp(0.0, 1.0))),
                (1.0, gdk::RGBA::new(1.0, 1.0, 1.0, 0.0)),
            ]),
        );
    }

    snapshot.pop();

    // Rim: a stroke masked out of a gradient whose brightness runs lit corner,
    // sides, far corner.
    let (border, edge) = (&m.border, &m.edge);
    if border.width > 0.0 && border.opacity > 0.0 {
        let (from, to) = if edge.follow_light {
            // Lit end faces the light source.
            let a = hl.azimuth.to_radians();
            let (sx, sy) = (a.sin(), -a.cos());
            let half = (sx.abs() * w + sy.abs() * h) / 2.0;
            let (cx, cy) = (w / 2.0, h / 2.0);
            (
                graphene::Point::new(cx + sx * half, cy + sy * half),
                graphene::Point::new(cx - sx * half, cy - sy * half),
            )
        } else {
            (graphene::Point::new(0.0, 0.0), graphene::Point::new(w, h))
        };
        let rw = border.width;
        let lit = gdk_rgba(edge.lit, edge.intensity * border.opacity * boost);
        let side = gdk_rgba(edge.side, border.opacity * boost);
        let far = gdk_rgba(edge.far, edge.intensity * border.opacity * boost);
        snapshot.push_mask(gsk::MaskMode::Alpha);
        snapshot.append_border(&shape, &[rw; 4], &[gdk::RGBA::new(0.0, 0.0, 0.0, 1.0); 4]);
        snapshot.pop();
        snapshot.append_linear_gradient(
            bounds,
            &from,
            &to,
            &stops(&[(0.0, lit), (edge.side_start, side), (edge.side_end, side), (1.0, far)]),
        );
        snapshot.pop();
    }
}

/// Eased progress of one animated value (see `advance`).
#[derive(Clone, Copy, Default)]
struct Tween {
    from: f32,
    target: f32,
    elapsed: f32,
}

/// Moves `value` towards `target` for one frame of `dt` seconds. Returns true
/// once it has arrived. [`Easing::Exponential`] approaches continuously (and
/// so retargets seamlessly); every other easing runs a timed tween that
/// restarts from the current value whenever the target changes.
fn advance(
    value: &mut f32,
    tween: &mut Tween,
    target: f32,
    duration_ms: f32,
    easing: Easing,
    dt: f32,
    epsilon: f32,
) -> bool {
    if duration_ms <= 0.0 {
        *value = target;
        return true;
    }
    if easing == Easing::Exponential {
        // A duration is the time to reach ~95 %, i.e. three time constants.
        let tau = (duration_ms / 3000.0).max(0.001);
        let k = 1.0 - (-dt / tau).exp();
        *value += (target - *value) * k;
        if (target - *value).abs() < epsilon {
            *value = target;
            return true;
        }
        return false;
    }
    if (tween.target - target).abs() > f32::EPSILON {
        tween.from = *value;
        tween.target = target;
        tween.elapsed = 0.0;
    }
    tween.elapsed += dt;
    let t = (tween.elapsed / (duration_ms / 1000.0)).clamp(0.0, 1.0);
    *value = tween.from + (target - tween.from) * easing.apply(t);
    if t >= 1.0 {
        *value = target;
        return true;
    }
    false
}

// ------------------------------------------------------------------- widget

fn is_dark(widget: &gtk::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if w.has_css_class("dark-mode") {
            return true;
        }
        current = w.parent();
    }
    false
}

mod imp {
    use super::*;

    pub struct GlassSurface {
        pub material: Cell<Material>,
        pub child: RefCell<Option<gtk::Widget>>,
        pub look: Cell<Look>,
        /// Timed-tween state for hover, press and on (used by non-exponential easings).
        pub tweens: Cell<[Tween; 3]>,
        pub hover_target: Cell<f32>,
        pub press_target: Cell<f32>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub last_frame: Cell<i64>,
        pub child_on_watch: RefCell<Option<glib::SignalHandlerId>>,
    }

    impl Default for GlassSurface {
        fn default() -> Self {
            Self {
                material: Cell::new(Material::clear()),
                child: RefCell::new(None),
                look: Cell::new(Look {
                    px: glass_config::INTERACTION.rest_pointer[0],
                    py: glass_config::INTERACTION.rest_pointer[1],
                    ..Look::default()
                }),
                tweens: Cell::new([Tween::default(); 3]),
                hover_target: Cell::new(0.0),
                press_target: Cell::new(0.0),
                tick: RefCell::new(None),
                last_frame: Cell::new(0),
                child_on_watch: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlassSurface {
        const NAME: &'static str = "LunarGlassSurface";
        type Type = super::GlassSurface;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_layout_manager_type::<gtk::BinLayout>();
            class.set_css_name("glasssurface");
        }
    }

    impl ObjectImpl for GlassSurface {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Pointer tracking in the capture phase so the child still gets
            // its own events untouched.
            let motion = gtk::EventControllerMotion::new();
            motion.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let obj = obj.downgrade();
                motion.connect_enter(move |_, x, y| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().pointer(&obj, x, y, true);
                    }
                });
            }
            {
                let obj = obj.downgrade();
                motion.connect_motion(move |_, x, y| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().pointer(&obj, x, y, true);
                    }
                });
            }
            {
                let obj = obj.downgrade();
                motion.connect_leave(move |_| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().hover_target.set(0.0);
                        obj.imp().press_target.set(0.0);
                        obj.imp().start_tick(&obj);
                    }
                });
            }
            obj.add_controller(motion);

            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.set_button(0);
            {
                let obj = obj.downgrade();
                click.connect_pressed(move |_, _, _, _| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().press_target.set(1.0);
                        obj.imp().start_tick(&obj);
                    }
                });
            }
            {
                let obj = obj.downgrade();
                click.connect_released(move |_, _, _, _| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().press_target.set(0.0);
                        obj.imp().start_tick(&obj);
                    }
                });
            }
            {
                let obj = obj.downgrade();
                click.connect_stopped(move |_| {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().press_target.set(0.0);
                        obj.imp().start_tick(&obj);
                    }
                });
            }
            obj.add_controller(click);
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
            if let Some(id) = self.tick.take() {
                id.remove();
            }
        }
    }

    impl GlassSurface {
        fn pointer(&self, obj: &super::GlassSurface, x: f64, y: f64, inside: bool) {
            if !self.material.get().interactive {
                return;
            }
            let (w, h) = (obj.width().max(1) as f32, obj.height().max(1) as f32);
            let mut look = self.look.get();
            look.px = (x as f32 / w).clamp(0.0, 1.0);
            look.py = (y as f32 / h).clamp(0.0, 1.0);
            self.look.set(look);
            self.hover_target.set(if inside { 1.0 } else { 0.0 });
            self.start_tick(obj);
            obj.queue_draw();
        }

        /// The `on` target follows the child's `.on` class.
        pub fn on_target(&self) -> f32 {
            let on = self.child.borrow().as_ref().is_some_and(|c| c.has_css_class("on"));
            if on {
                1.0
            } else {
                0.0
            }
        }

        pub fn start_tick(&self, obj: &super::GlassSurface) {
            if self.tick.borrow().is_some() {
                return;
            }
            self.last_frame.set(0);
            let id = obj.add_tick_callback(|obj, clock| {
                let imp = obj.imp();
                let now = clock.frame_time();
                let dt = if imp.last_frame.get() == 0 {
                    1.0 / 60.0
                } else {
                    ((now - imp.last_frame.get()) as f32 / 1_000_000.0).clamp(0.0, 0.1)
                };
                imp.last_frame.set(now);

                let motion = imp.material.get().tuned().motion;
                let epsilon = glass_config::INTERACTION.settle_epsilon;
                let mut look = imp.look.get();
                let mut tweens = imp.tweens.get();
                // Hover and press use their own durations for going in and out;
                // all of them come from the material's `Motion`.
                let hover_target = imp.hover_target.get();
                let press_target = imp.press_target.get();
                let hover_ms = if hover_target > look.hover { motion.hover_in_ms } else { motion.hover_out_ms };
                let press_ms = if press_target > look.press { motion.press_in_ms } else { motion.press_out_ms };
                let mut settled = true;
                settled &= advance(&mut look.hover, &mut tweens[0], hover_target, hover_ms, motion.easing, dt, epsilon);
                settled &= advance(&mut look.press, &mut tweens[1], press_target, press_ms, motion.easing, dt, epsilon);
                settled &= advance(&mut look.on, &mut tweens[2], imp.on_target(), motion.toggle_ms, motion.easing, dt, epsilon);
                imp.tweens.set(tweens);
                imp.look.set(look);
                obj.queue_draw();
                if settled {
                    imp.tick.replace(None);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            self.tick.replace(Some(id));
        }
    }

    impl WidgetImpl for GlassSurface {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let widget = obj.upcast_ref::<gtk::Widget>();
            let material = self.material.get().tuned().for_dark(is_dark(widget));
            let ix = glass_config::INTERACTION;
            let mut look = self.look.get();
            // Pick up an `.on` change that arrived without a tick running.
            let on_target = self.on_target();
            if (look.on - on_target).abs() > ix.settle_epsilon {
                self.start_tick(&obj);
            }
            // Easings that overshoot must not push the state out of range.
            look.on = look.on.clamp(0.0, 1.0);
            look.hover = look.hover.max(0.0);
            look.press = look.press.max(0.0);

            let bounds = graphene::Rect::new(0.0, 0.0, w, h);
            // Where this surface sits in its window, for the preview backdrop.
            let origin = widget
                .root()
                .and_then(|root| widget.compute_point(root.upcast_ref::<gtk::Widget>(), &graphene::Point::new(0.0, 0.0)))
                .map_or(graphene::Rect::new(0.0, 0.0, w, h), |p| graphene::Rect::new(p.x(), p.y(), w, h));

            // Hover lifts, press compresses; both pivot on the centre.
            let scale = if material.interactive {
                1.0 + ix.hover_scale * look.hover - ix.press_scale * look.press
            } else {
                1.0
            };
            snapshot.save();
            if (scale - 1.0).abs() > 0.0005 {
                snapshot.translate(&graphene::Point::new(w / 2.0, h / 2.0));
                snapshot.scale(scale, scale);
                snapshot.translate(&graphene::Point::new(-w / 2.0, -h / 2.0));
            }
            paint_under(snapshot, &bounds, &material, look, &origin);
            if let Some(child) = self.child.borrow().as_ref() {
                obj.snapshot_child(child, snapshot);
            }
            paint_over(snapshot, &bounds, &material, look);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    /// A container that paints its single child on a Liquid Glass surface.
    pub struct GlassSurface(ObjectSubclass<imp::GlassSurface>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GlassSurface {
    pub fn new(material: Material) -> Self {
        let surface: Self = glib::Object::new();
        surface.imp().material.set(material);
        surface
    }

    /// Puts `child` on a glass surface that takes over the child's alignment,
    /// expansion and margins, so swapping it into an existing layout leaves
    /// the layout unchanged.
    pub fn wrap(child: &impl IsA<gtk::Widget>, material: Material) -> Self {
        let child = child.as_ref();
        let surface = Self::new(material);
        surface.set_halign(child.halign());
        surface.set_valign(child.valign());
        surface.set_hexpand(child.hexpands());
        surface.set_vexpand(child.vexpands());
        surface.set_margin_top(child.margin_top());
        surface.set_margin_bottom(child.margin_bottom());
        surface.set_margin_start(child.margin_start());
        surface.set_margin_end(child.margin_end());
        let (w, h) = child.size_request();
        surface.set_size_request(w, h);
        child.set_margin_top(0);
        child.set_margin_bottom(0);
        child.set_margin_start(0);
        child.set_margin_end(0);
        child.set_halign(gtk::Align::Fill);
        child.set_valign(gtk::Align::Fill);
        surface.set_child(Some(child));
        surface
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        let imp = self.imp();
        if let Some(old) = imp.child.take() {
            old.unparent();
        }
        if let Some(child) = child {
            let child = child.as_ref().clone();
            child.set_parent(self);
            // A class change on the child (`.on`) animates the fill.
            let weak = self.downgrade();
            child.connect_css_classes_notify(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.imp().start_tick(&this);
                }
            });
            imp.child.replace(Some(child));
        }
        self.queue_resize();
    }

    pub fn child(&self) -> Option<gtk::Widget> {
        self.imp().child.borrow().clone()
    }

    /// Freezes the interaction state (used by the preview harness to render
    /// hover and press without a real pointer).
    #[allow(dead_code)]
    pub fn set_look_for_preview(&self, hover: f32, press: f32, px: f32, py: f32) {
        let imp = self.imp();
        let mut look = imp.look.get();
        look.hover = hover;
        look.press = press;
        look.px = px;
        look.py = py;
        imp.look.set(look);
        imp.hover_target.set(hover);
        imp.press_target.set(press);
        self.queue_draw();
    }

    pub fn set_material(&self, material: Material) {
        self.imp().material.set(material);
        self.queue_draw();
    }
}

/// Replaces a popover's child with the same child on a glass surface, and
/// returns the surface. The blur regions registered against the child stay
/// valid because the child keeps the surface's exact bounds.
pub fn glass_popover(popover: &gtk::Popover, material: Material) -> Option<GlassSurface> {
    let child = popover.child()?;
    popover.set_child(None::<&gtk::Widget>);
    let surface = GlassSurface::wrap(&child, material);
    popover.set_child(Some(&surface));
    Some(surface)
}
