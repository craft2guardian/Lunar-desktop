//! Liquid Glass configuration: the single place that defines how glass looks.
//!
//! Nothing else in the shell hardcodes a glass colour, opacity, radius, shadow,
//! highlight or animation timing. Painters (`liquid_glass.rs`, `glass.rs`), the
//! compositor blur regions and the glass-coupled CSS all read from here.
//!
//! # How to use this file
//!
//! * **Fine-tune everything at once** by editing [`TUNING`] (a handful of
//!   multipliers; `1.0` = the shipped design, so you only list what you change).
//! * **Restyle one material** by editing its preset in [`presets`]. Each preset
//!   is split into the same sections: body, blur, refraction, highlight,
//!   reflection, border, edge light, shadow, depth, motion.
//! * **Change behaviour shared by all glass** (hover/press response, dark-mode
//!   adaptation, popover accents) in [`INTERACTION`], [`DARK`] and [`MENU_CHROME`].
//! * **Add a material** by adding a preset constant and a `Preset::name()`
//!   constructor, then use `Material::name()` wherever you need it.
//!
//! Design values live in the presets; [`TUNING`] is applied on top of them at
//! paint time by [`Preset::tuned`], so a changed value reaches every surface.
//!
//! # What each section really does
//!
//! GTK cannot warp or blur what the compositor draws behind a window, so some
//! optical effects are *simulated* and some are *delegated*:
//!
//! | Section      | Rendered by | Notes |
//! |--------------|-------------|-------|
//! | `body`       | GTK         | Tint + opacity of the glass slab, saturation of the tint. |
//! | `blur`       | KWin / GTK  | KWin's blur *strength* is a global compositor setting. `radius` drives app-side blur (the preview backdrop); `frost` adds a haze over KWin's blur, which is the app-side way to read as "more diffuse". |
//! | `refraction` | GTK         | The meniscus: an inner luminous band where a thick lens bends the backdrop most. |
//! | `highlight`  | GTK         | Directional specular lines and the cursor glint. `azimuth` positions the light. |
//! | `reflection` | GTK         | Broad sheen over the upper part (the slab catching the sky). |
//! | `border`     | GTK         | Rim stroke width and overall opacity. |
//! | `edge`       | GTK         | Rim brightness: lit corner, sides, far corner. |
//! | `shadow`     | GTK + CSS   | Ambient + contact shadow. CSS margins that reserve room for it are derived. |

// ================================================================== basics

/// Linear-ish sRGB + alpha, each channel `0.0..=1.0`.
pub type Rgba = [f32; 4];

/// `rgba(r, g, b, a)` with 8-bit colour channels and a `0.0..=1.0` alpha.
pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

/// Linear interpolation between two colours (alpha included), `t` clamped.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Scales a colour's saturation around its luminance (`1.0` = unchanged).
pub fn saturate(c: Rgba, amount: f32) -> Rgba {
    let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let adjust = |v: f32| (luma + (v - luma) * amount).clamp(0.0, 1.0);
    [adjust(c[0]), adjust(c[1]), adjust(c[2]), c[3]]
}

/// How rounded a surface is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Radius {
    /// A fixed radius in logical pixels (clamped to half the shorter side).
    Fixed(f32),
    /// Fully round ends: a circle or a capsule.
    Capsule,
}

/// Easing used for glass interaction animation (hover, press, switch on/off).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    /// Frame-rate independent exponential approach. Retargets smoothly mid-flight,
    /// which is why it is the default for pointer-driven motion. A duration is the
    /// time to reach about 95 % of the way.
    Exponential,
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    EaseOutQuint,
    /// Overshoots slightly and settles: a springy, liquid feel.
    EaseOutBack,
}

impl Easing {
    /// Maps linear progress `0..=1` to eased progress (`0..=1`, may overshoot).
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Exponential => (1.0 - (-3.0 * t).exp()) / (1.0 - (-3.0f32).exp()),
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::EaseOutQuint => 1.0 - (1.0 - t).powi(5),
            Easing::EaseOutBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                let u = t - 1.0;
                1.0 + c3 * u * u * u + c1 * u * u
            }
        }
    }

    /// The matching CSS timing function, for glass-coupled CSS transitions.
    pub const fn css(self) -> &'static str {
        match self {
            Easing::Exponential => "ease-out",
            Easing::Linear => "linear",
            Easing::EaseOutCubic => "cubic-bezier(0.215, 0.61, 0.355, 1)",
            Easing::EaseInOutCubic => "cubic-bezier(0.645, 0.045, 0.355, 1)",
            Easing::EaseOutQuint => "cubic-bezier(0.23, 1, 0.32, 1)",
            Easing::EaseOutBack => "cubic-bezier(0.175, 0.885, 0.32, 1.275)",
        }
    }
}

// ================================================================== tuning

/// Global fine-tuning knobs. Every value multiplies (or offsets) the design
/// values of *every* preset, so one edit here changes the whole shell
/// consistently. `1.0` (or `0.0` for additive knobs) means "as designed".
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Glass opacity: multiplies the alpha of every body fill (and the menu-bar
    /// tint). Lower = more transparent glass.
    pub opacity: f32,
    /// Saturation of the glass tint (`0.0` = grey, `1.0` = designed, `>1` = vivid).
    pub tint_saturation: f32,
    /// Multiplier on app-side blur radii (see the table in the module docs).
    pub blur_radius: f32,
    /// Extra frost haze (additive alpha, `0.0..=1.0`) painted over the backdrop
    /// blur. KWin's own blur strength cannot be changed per app, so this is how
    /// a surface reads as more diffuse.
    pub frost: f32,
    /// Strength of the refraction meniscus (edge glow brightness and width).
    pub refraction: f32,
    /// Strength of specular highlights and the cursor glint.
    pub highlight: f32,
    /// Strength of the broad upper reflection (sheen).
    pub reflection: f32,
    /// Overall rim/border opacity.
    pub border_opacity: f32,
    /// Rim/border thickness.
    pub border_width: f32,
    /// Directional edge lighting (brightness of the lit and far corners).
    pub edge_light: f32,
    /// Shadow intensity.
    pub shadow_intensity: f32,
    /// Shadow softness (multiplies blur radii).
    pub shadow_softness: f32,
    /// Extra ambient shadow spread, in logical pixels (additive).
    pub shadow_spread: f32,
    /// Corner radius multiplier (applies to painting, blur regions and CSS).
    pub corner_radius: f32,
    /// Material depth: thickness of the slab. Scales shadow offset and blur,
    /// meniscus width and the distance of specular lines.
    pub depth: f32,
    /// Rotates the light source, in degrees (additive; positive = clockwise).
    pub light_angle: f32,
    /// Animation speed (`2.0` = twice as fast, `0.5` = half speed).
    pub animation_speed: f32,
    /// Forces one easing curve for all glass motion. `None` keeps each preset's own.
    pub easing: Option<Easing>,
}

/// The "as designed" tuning. Used as the base for [`TUNING`] and by the tests.
pub const NEUTRAL: Tuning = Tuning {
    opacity: 1.0,
    tint_saturation: 0.30,
    blur_radius: 1.0,
    frost: 0.0,
    refraction: 1.0,
    highlight: 1.0,
    reflection: 1.0,
    border_opacity: 1.0,
    border_width: 1.0,
    edge_light: 1.0,
    shadow_intensity: 1.0,
    shadow_softness: 1.0,
    shadow_spread: 0.0,
    corner_radius: 1.0,
    depth: 1.0,
    light_angle: 0.0,
    animation_speed: 1.0,
    easing: None,
};

/// ===== EDIT ME =====  List only what you want to change, e.g.
///
/// ```ignore
/// pub const TUNING: Tuning = Tuning {
///     opacity: 0.85,        // clearer glass
///     refraction: 1.4,      // stronger edge lensing
///     corner_radius: 1.2,   // rounder everything
///     animation_speed: 1.5, // snappier
///     easing: Some(Easing::EaseOutBack),
///     ..NEUTRAL
/// };
/// ```
pub const TUNING: Tuning = Tuning { ..NEUTRAL };

// ================================================================ sections

/// The slab itself: tint, opacity, saturation.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    /// Fill at the top and bottom edge (a vertical gradient between them).
    pub top: Rgba,
    pub bottom: Rgba,
    /// Fill when the surface is switched on (its child carries `.on`).
    pub on_top: Rgba,
    pub on_bottom: Rgba,
    /// Per-preset alpha multiplier (on top of [`Tuning::opacity`]).
    pub opacity: f32,
    /// Per-preset saturation (on top of [`Tuning::tint_saturation`]).
    pub saturation: f32,
    /// Body the surface thins to on hover when `hover_clears` is set.
    pub hover_clear_top: Rgba,
    pub hover_clear_bottom: Rgba,
    /// How far (0..1) the body moves towards the hover-clear body.
    pub hover_clear_amount: f32,
}

impl Body {
    pub const DEFAULT: Body = Body {
        top: rgba(255, 255, 255, 0.0),
        bottom: rgba(255, 255, 255, 0.0),
        on_top: rgba(255, 255, 255, 0.0),
        on_bottom: rgba(255, 255, 255, 0.0),
        opacity: 1.0,
        saturation: 1.0,
        hover_clear_top: rgba(190, 214, 240, 0.34),
        hover_clear_bottom: rgba(120, 160, 214, 0.34),
        hover_clear_amount: 0.9,
    };
}

/// Backdrop blur. See the module docs: KWin owns the real blur strength.
#[derive(Clone, Copy, Debug)]
pub struct Blur {
    /// Radius (logical px) of app-side blur, e.g. the preview harness backdrop.
    pub radius: f32,
    /// Haze painted over the backdrop; the alpha is the amount.
    pub frost: Rgba,
}

impl Blur {
    pub const DEFAULT: Blur = Blur { radius: 26.0, frost: rgba(255, 255, 255, 0.0) };
}

/// Refraction at the rim: the meniscus glow.
#[derive(Clone, Copy, Debug)]
pub struct Refraction {
    /// Colour and peak alpha of the inner edge glow.
    pub glow: Rgba,
    /// Width of the band, as a fraction of the corner radius.
    pub reach: f32,
    /// Width of the tight second band, as a fraction of the wide one.
    pub tight_reach: f32,
    /// Alpha scale of the tight band.
    pub tight_alpha: f32,
    /// Limits of the wide band in logical px.
    pub reach_min: f32,
    pub reach_max: f32,
    /// Minimum width of the tight band in logical px.
    pub tight_min: f32,
}

impl Refraction {
    pub const DEFAULT: Refraction = Refraction {
        glow: rgba(255, 255, 255, 0.0),
        reach: 0.5,
        tight_reach: 0.3,
        tight_alpha: 0.7,
        reach_min: 3.0,
        reach_max: 18.0,
        tight_min: 1.2,
    };
}

/// Specular highlights: the key light, its weaker bounce, and the cursor glint.
#[derive(Clone, Copy, Debug)]
pub struct Highlight {
    /// Overall strength (0..1) of the specular lines.
    pub strength: f32,
    /// Where the light comes from, in degrees clockwise from the top
    /// (`0` = above, `90` = right, `270` = left, `320` = upper left).
    pub azimuth: f32,
    /// Key light line: alpha, distance it is pushed in from the edge (px), softness (px).
    pub key_alpha: f32,
    pub key_distance: f32,
    pub key_softness: f32,
    /// Bounce light on the opposite edge.
    pub bounce_alpha: f32,
    pub bounce_distance: f32,
    pub bounce_softness: f32,
    /// Cursor glint: alpha at the centre and at the mid stop, radius as a
    /// fraction of the longer side, and a minimum radius in px.
    pub glint_alpha: f32,
    pub glint_falloff_alpha: f32,
    pub glint_radius: f32,
    pub glint_min_radius: f32,
}

impl Highlight {
    pub const DEFAULT: Highlight = Highlight {
        strength: 0.0,
        azimuth: 320.0,
        key_alpha: 0.62,
        key_distance: 1.7,
        key_softness: 1.2,
        bounce_alpha: 0.28,
        bounce_distance: 1.5,
        bounce_softness: 1.4,
        glint_alpha: 0.26,
        glint_falloff_alpha: 0.08,
        glint_radius: 0.6,
        glint_min_radius: 30.0,
    };
}

/// Reflection: the broad sheen over the upper part of the surface.
#[derive(Clone, Copy, Debug)]
pub struct Reflection {
    /// Overall strength (0..1).
    pub strength: f32,
    /// How much of the height the sheen covers (0..1), measured from the top.
    pub coverage: f32,
    /// White alpha at the top and at `mid_stop`.
    pub top_alpha: f32,
    pub mid_alpha: f32,
    pub mid_stop: f32,
}

impl Reflection {
    pub const DEFAULT: Reflection =
        Reflection { strength: 0.0, coverage: 0.62, top_alpha: 0.20, mid_alpha: 0.05, mid_stop: 0.55 };
}

/// The rim stroke.
#[derive(Clone, Copy, Debug)]
pub struct Border {
    /// Thickness in logical px.
    pub width: f32,
    /// Overall opacity multiplier.
    pub opacity: f32,
}

impl Border {
    pub const DEFAULT: Border = Border { width: 1.0, opacity: 1.0 };
}

/// Rim lighting: brightness along the stroke.
#[derive(Clone, Copy, Debug)]
pub struct EdgeLight {
    /// Rim colour at the lit corner, along the sides, and at the far corner.
    pub lit: Rgba,
    pub side: Rgba,
    pub far: Rgba,
    /// Scales the lit and far corners (not the sides): contrast of the lighting.
    pub intensity: f32,
    /// Where the side colour starts and ends along the gradient.
    pub side_start: f32,
    pub side_end: f32,
    /// `false`: the gradient runs corner to corner. `true`: it follows
    /// [`Highlight::azimuth`], so the lit rim faces the light.
    pub follow_light: bool,
}

impl EdgeLight {
    pub const DEFAULT: EdgeLight = EdgeLight {
        lit: rgba(255, 255, 255, 0.0),
        side: rgba(255, 255, 255, 0.0),
        far: rgba(255, 255, 255, 0.0),
        intensity: 1.0,
        side_start: 0.34,
        side_end: 0.66,
        follow_light: false,
    };
}

/// One drop-shadow layer.
#[derive(Clone, Copy, Debug)]
pub struct ShadowLayer {
    pub alpha: f32,
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
}

/// Two-stage drop shadow: a broad ambient shadow and a tight contact shadow.
#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    /// Overall intensity (0..1+). `0.0` draws no shadow at all.
    pub intensity: f32,
    pub color: [f32; 3],
    pub ambient: ShadowLayer,
    pub contact: ShadowLayer,
}

impl Shadow {
    pub const DEFAULT: Shadow = Shadow {
        intensity: 0.0,
        color: [0.02, 0.05, 0.14],
        ambient: ShadowLayer { alpha: 0.20, dx: 0.0, dy: 9.0, blur: 26.0, spread: 0.0 },
        contact: ShadowLayer { alpha: 0.14, dx: 0.0, dy: 1.5, blur: 4.0, spread: 0.0 },
    };
}

/// Glass interaction timing. Durations are in milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub hover_in_ms: f32,
    pub hover_out_ms: f32,
    pub press_in_ms: f32,
    pub press_out_ms: f32,
    /// Crossfade when a switch turns on or off.
    pub toggle_ms: f32,
    pub easing: Easing,
}

impl Motion {
    /// With [`Easing::Exponential`] a duration is the time to reach ~95 %, so
    /// these correspond to time constants of 70 / 140 / 45 / 120 / 110 ms.
    pub const DEFAULT: Motion = Motion {
        hover_in_ms: 210.0,
        hover_out_ms: 420.0,
        press_in_ms: 135.0,
        press_out_ms: 360.0,
        toggle_ms: 330.0,
        easing: Easing::Exponential,
    };
}

// ================================================================== presets

/// A complete glass material: one value for every section.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub radius: Radius,
    pub body: Body,
    pub blur: Blur,
    pub refraction: Refraction,
    pub highlight: Highlight,
    pub reflection: Reflection,
    pub border: Border,
    pub edge: EdgeLight,
    pub shadow: Shadow,
    /// Thickness of the slab (`1.0` = designed).
    pub depth: f32,
    pub motion: Motion,
    /// Hover/press response (lift, glint, compression).
    pub interactive: bool,
    /// On hover the body thins out into clear glass and lets the backdrop through.
    pub hover_clears: bool,
    /// Whether the body deepens in Dark Mode. Accent glass keeps its colour.
    pub dark_adapts: bool,
}

/// The design values of every material. Edit these to restyle one material.
pub mod presets {
    use super::*;

    /// Radius shared by the Control Centre's large tiles.
    pub const CARD_RADIUS: f32 = 24.0;

    /// Clear glass: almost no body colour, so the backdrop shows through. The
    /// Control Centre tiles. Clear glass absorbs a little: slightly darker and
    /// cooler than what is behind it, with the light collected at the edge.
    pub const CLEAR: Preset = Preset {
        radius: Radius::Fixed(22.0),
        body: Body {
            top: rgba(40, 150, 176, 0.20),
            bottom: rgba(14, 106, 156, 0.22),
            on_top: rgba(10, 132, 255, 0.92),
            on_bottom: rgba(0, 96, 245, 0.95),
            ..Body::DEFAULT
        },
        blur: Blur::DEFAULT,
        refraction: Refraction { glow: rgba(150, 240, 255, 0.34), reach: 0.5, ..Refraction::DEFAULT },
        highlight: Highlight { strength: 0.8, ..Highlight::DEFAULT },
        reflection: Reflection { strength: 0.16, ..Reflection::DEFAULT },
        border: Border::DEFAULT,
        edge: EdgeLight {
            lit: rgba(226, 252, 255, 0.98),
            side: rgba(150, 232, 248, 0.62),
            far: rgba(200, 248, 255, 0.86),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow { intensity: 0.40, ..Shadow::DEFAULT },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: true,
        hover_clears: false,
        dark_adapts: true,
    };

    /// Accent glass: saturated blue slabs (sliders, shortcut buttons).
    pub const ACCENT: Preset = Preset {
        radius: Radius::Fixed(22.0),
        body: Body {
            top: rgba(12, 122, 255, 0.93),
            bottom: rgba(0, 82, 250, 0.96),
            on_top: rgba(12, 122, 255, 0.93),
            on_bottom: rgba(0, 82, 250, 0.96),
            ..Body::DEFAULT
        },
        blur: Blur::DEFAULT,
        refraction: Refraction { glow: rgba(120, 204, 255, 0.52), reach: 0.6, ..Refraction::DEFAULT },
        highlight: Highlight { strength: 0.7, ..Highlight::DEFAULT },
        reflection: Reflection { strength: 0.4, ..Reflection::DEFAULT },
        border: Border::DEFAULT,
        edge: EdgeLight {
            lit: rgba(172, 226, 255, 0.95),
            side: rgba(110, 196, 255, 0.64),
            far: rgba(140, 214, 255, 0.82),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow { intensity: 0.45, ..Shadow::DEFAULT },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: true,
        hover_clears: true,
        dark_adapts: false,
    };

    /// Menus and popovers, tuned to match a macOS menu: a *thin, dark, neutral*
    /// wash over the blurred backdrop so the backdrop's colour still comes
    /// through (macOS darkens what is behind a menu by roughly 10-20 % and keeps
    /// its hue; it does not lay a milky grey sheet over it). There is no sheen,
    /// no specular line and no bright rim: the only edge detail is a very faint
    /// inner light line plus a thin dark hairline outside (the contact shadow).
    pub const MENU: Preset = Preset {
        radius: Radius::Fixed(12.0),
        // macOS: a dark, slightly cool neutral wash that lets the blurred
        // backdrop's colour read through (about 40-45 % opaque).
        body: Body {
            top: rgba(40, 38, 52, 0.40),
            bottom: rgba(32, 32, 46, 0.46),
            on_top: rgba(40, 38, 52, 0.40),
            on_bottom: rgba(32, 32, 46, 0.46),
            ..Body::DEFAULT
        },
        blur: Blur::DEFAULT,
        // A faint inner edge glow, as on a thick glass slab.
        refraction: Refraction {
            glow: rgba(255, 255, 255, 0.07),
            reach: 0.45,
            ..Refraction::DEFAULT
        },
        // A thin, soft key-light line along the upper-left inner edge.
        highlight: Highlight {
            strength: 0.14,
            ..Highlight::DEFAULT
        },
        // A barely-there sheen over the top of the menu.
        reflection: Reflection {
            strength: 0.06,
            coverage: 0.45,
            ..Reflection::DEFAULT
        },
        border: Border { width: 1.0, opacity: 1.0 },
        // The visible 1px light rim around the whole menu, brightest at the
        // upper-left and softest at the lower-right.
        edge: EdgeLight {
            lit: rgba(255, 255, 255, 0.36),
            side: rgba(255, 255, 255, 0.20),
            far: rgba(255, 255, 255, 0.16),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow {
            intensity: 0.85,
            color: [0.0, 0.0, 0.03],
            ambient: ShadowLayer { alpha: 0.30, dx: 0.0, dy: 8.0, blur: 22.0, spread: 0.0 },
            // ~1px dark hairline hugging the menu, outside the light rim.
            contact: ShadowLayer { alpha: 0.60, dx: 0.0, dy: 0.0, blur: 1.0, spread: 0.5 },
        },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: false,
        hover_clears: false,
        dark_adapts: true,
    };

    /// Menus that are *not* blurred by the compositor (context menus and the
    /// Settings drop-downs). Same look as [`MENU`], but with a nearly opaque
    /// body: the thin wash only reads well over a blurred backdrop, and
    /// without blur it would let the desktop show straight through.
    pub const MENU_SOLID: Preset = Preset {
        radius: Radius::Fixed(12.0),
        body: Body {
            top: rgba(38, 38, 46, 0.97),
            bottom: rgba(32, 32, 40, 0.97),
            on_top: rgba(38, 38, 46, 0.97),
            on_bottom: rgba(32, 32, 40, 0.97),
            ..Body::DEFAULT
        },
        blur: Blur::DEFAULT,
        refraction: Refraction {
            glow: rgba(255, 255, 255, 0.03),
            reach: 0.5,
            ..Refraction::DEFAULT
        },
        highlight: Highlight {
            strength: 0.05,
            ..Highlight::DEFAULT
        },
        reflection: Reflection {
            strength: 0.0,
            ..Reflection::DEFAULT
        },
        border: Border::DEFAULT,
        edge: EdgeLight {
            lit: rgba(255, 255, 255, 0.10),
            side: rgba(255, 255, 255, 0.04),
            far: rgba(255, 255, 255, 0.06),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow {
            intensity: 0.75,
            color: [0.0, 0.0, 0.02],
            contact: ShadowLayer { alpha: 0.55, dx: 0.0, dy: 0.0, blur: 1.0, spread: 0.5 },
            ..Shadow::DEFAULT
        },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: false,
        hover_clears: false,
        dark_adapts: true,
    };

    /// The Dock shelf: only the outer border is painted here. The glass itself
    /// (blur, refraction, tint) comes from the KWin glass plugin, so the body,
    /// meniscus glow, specular lines, sheen and drop shadow are all off.
    pub const DOCK: Preset = Preset {
        radius: Radius::Fixed(26.0),
        body: Body::DEFAULT,
        blur: Blur::DEFAULT,
        refraction: Refraction::DEFAULT,
        highlight: Highlight { strength: 0.0, ..Highlight::DEFAULT },
        reflection: Reflection { strength: 0.0, ..Reflection::DEFAULT },
        border: Border::DEFAULT,
        edge: EdgeLight {
            lit: rgba(255, 255, 255, 0.35),
            side: rgba(255, 255, 255, 0.12),
            far: rgba(255, 255, 255, 0.20),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow { intensity: 0.0, ..Shadow::DEFAULT },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: false,
        hover_clears: false,
        dark_adapts: true,
    };

    /// The small label bubble shown above a Dock icon: a menu with a smaller
    /// radius and a slightly stronger tint.
    pub const BUBBLE: Preset = Preset {
        radius: Radius::Fixed(9.0),
        body: Body {
            top: rgba(70, 92, 126, 0.46),
            bottom: rgba(54, 74, 108, 0.52),
            on_top: rgba(70, 92, 126, 0.46),
            on_bottom: rgba(54, 74, 108, 0.52),
            ..Body::DEFAULT
        },
        ..MENU
    };

    /// Window sidebars and panels.
    pub const SIDEBAR: Preset = Preset {
        radius: Radius::Fixed(18.0),
        body: Body {
            top: rgba(246, 248, 252, 0.74),
            bottom: rgba(236, 240, 247, 0.78),
            on_top: rgba(246, 248, 252, 0.74),
            on_bottom: rgba(236, 240, 247, 0.78),
            ..Body::DEFAULT
        },
        blur: Blur::DEFAULT,
        refraction: Refraction { glow: rgba(255, 255, 255, 0.35), reach: 0.35, ..Refraction::DEFAULT },
        highlight: Highlight { strength: 0.7, ..Highlight::DEFAULT },
        reflection: Reflection { strength: 0.35, ..Reflection::DEFAULT },
        border: Border::DEFAULT,
        edge: EdgeLight {
            lit: rgba(255, 255, 255, 0.95),
            side: rgba(255, 255, 255, 0.40),
            far: rgba(255, 255, 255, 0.70),
            ..EdgeLight::DEFAULT
        },
        shadow: Shadow { intensity: 0.0, ..Shadow::DEFAULT },
        depth: 1.0,
        motion: Motion::DEFAULT,
        interactive: false,
        hover_clears: false,
        dark_adapts: true,
    };
}

impl Preset {
    pub const fn clear() -> Self {
        presets::CLEAR
    }
    pub const fn accent() -> Self {
        presets::ACCENT
    }
    pub const fn menu() -> Self {
        presets::MENU
    }
    /// The menu look without compositor blur (see [`presets::MENU_SOLID`]).
    pub const fn menu_solid() -> Self {
        presets::MENU_SOLID
    }
    pub const fn dock() -> Self {
        presets::DOCK
    }
    pub const fn bubble() -> Self {
        presets::BUBBLE
    }
    pub const fn sidebar() -> Self {
        presets::SIDEBAR
    }

    // Control Centre variants: named here so no component hardcodes a radius.
    pub const fn clear_capsule() -> Self {
        Self::clear().with_radius(Radius::Capsule)
    }
    pub const fn clear_card() -> Self {
        Self::clear().with_radius(Radius::Fixed(presets::CARD_RADIUS)).interactive(false)
    }
    pub const fn accent_capsule() -> Self {
        Self::accent().with_radius(Radius::Capsule)
    }
    pub const fn accent_card() -> Self {
        Self::accent().with_radius(Radius::Fixed(presets::CARD_RADIUS)).interactive(false)
    }

    pub const fn with_radius(mut self, radius: Radius) -> Self {
        self.radius = radius;
        self
    }

    pub const fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Resolves this design with the global [`TUNING`] applied.
    pub fn tuned(&self) -> Preset {
        self.tuned_with(&TUNING)
    }

    /// Resolves this design with an explicit tuning (see [`Preset::tuned`]).
    pub fn tuned_with(&self, t: &Tuning) -> Preset {
        let mut p = *self;
        let depth = (p.depth * t.depth).max(0.0);

        // Shape.
        if let Radius::Fixed(r) = p.radius {
            p.radius = Radius::Fixed((r * t.corner_radius).max(0.0));
        }

        // Body: opacity scales alpha, saturation acts on the tint.
        let saturation = t.tint_saturation * p.body.saturation;
        let alpha = t.opacity * p.body.opacity;
        for c in [
            &mut p.body.top,
            &mut p.body.bottom,
            &mut p.body.on_top,
            &mut p.body.on_bottom,
            &mut p.body.hover_clear_top,
            &mut p.body.hover_clear_bottom,
        ] {
            *c = saturate(*c, saturation);
            c[3] *= alpha;
        }

        // Blur.
        p.blur.radius = (p.blur.radius * t.blur_radius).max(0.0);
        p.blur.frost[3] = (p.blur.frost[3] + t.frost).clamp(0.0, 1.0);

        // Refraction (meniscus).
        p.refraction.glow[3] *= t.refraction;
        p.refraction.reach *= t.refraction.max(0.0).sqrt() * depth;

        // Highlight.
        p.highlight.strength *= t.highlight;
        p.highlight.glint_alpha *= t.highlight;
        p.highlight.glint_falloff_alpha *= t.highlight;
        p.highlight.azimuth += t.light_angle;
        p.highlight.key_distance *= depth;
        p.highlight.bounce_distance *= depth;

        // Reflection.
        p.reflection.strength *= t.reflection;

        // Border and edge lighting.
        p.border.width = (p.border.width * t.border_width).max(0.0);
        p.border.opacity *= t.border_opacity;
        p.edge.intensity *= t.edge_light;

        // Shadow: depth lifts the slab (longer, wider shadow); softness blurs it.
        p.shadow.intensity *= t.shadow_intensity;
        for layer in [&mut p.shadow.ambient, &mut p.shadow.contact] {
            layer.dx *= depth;
            layer.dy *= depth;
            layer.blur = (layer.blur * t.shadow_softness * depth).max(0.0);
        }
        p.shadow.ambient.spread += t.shadow_spread;

        // Motion.
        let speed = t.animation_speed.max(0.01);
        p.motion.hover_in_ms /= speed;
        p.motion.hover_out_ms /= speed;
        p.motion.press_in_ms /= speed;
        p.motion.press_out_ms /= speed;
        p.motion.toggle_ms /= speed;
        if let Some(easing) = t.easing {
            p.motion.easing = easing;
        }
        p
    }

    /// Dark appearance: deeper body, softer lights. Call on a tuned preset.
    pub fn for_dark(mut self, dark: bool) -> Self {
        if !dark || !self.dark_adapts {
            return self;
        }
        let d = DARK;
        let top = [d.night[0], d.night[1], d.night[2], self.body.top[3] + d.top_alpha_add];
        let bottom = [d.night[0], d.night[1], d.night[2], self.body.bottom[3] + d.bottom_alpha_add];
        self.body.top = mix(self.body.top, top, d.top_mix);
        self.body.bottom = mix(self.body.bottom, bottom, d.bottom_mix);
        self.edge.side[3] *= d.rim_side_scale;
        self.edge.lit[3] *= d.rim_lit_scale;
        self.refraction.glow[3] *= d.glow_scale;
        self.highlight.strength *= d.specular_scale;
        self
    }

    /// Corner radius to give the compositor's blur region so it matches the
    /// painted shape exactly. Capsules return a large radius that the blur code
    /// clamps to half the shorter side.
    pub fn blur_corner(&self) -> i32 {
        match self.tuned().radius {
            Radius::Fixed(r) => r.round() as i32,
            Radius::Capsule => CAPSULE_BLUR_RADIUS,
        }
    }
}

/// What the blur code is given for a capsule; it clamps to half the shorter side.
pub const CAPSULE_BLUR_RADIUS: i32 = 512;

/// The shipped design values are called "materials" throughout the shell.
pub type Material = Preset;

// ====================================================== shared behaviour

/// Hover / press response shared by every interactive glass surface.
#[derive(Clone, Copy, Debug)]
pub struct Interaction {
    /// Scale gained on hover and lost on press (fractions of the size).
    pub hover_scale: f32,
    pub press_scale: f32,
    /// Extra brightness of shadow, rim, specular and sheen at full hover.
    pub hover_boost: f32,
    /// Extra brightness of the meniscus glow at full hover.
    pub glow_hover_boost: f32,
    /// White lift added to the body at full hover.
    pub fill_hover_lift: f32,
    /// Darkening at full press, and its colour.
    pub press_dim: f32,
    pub press_dim_color: [f32; 3],
    /// Where the glint rests before the pointer arrives (0..1, 0..1).
    pub rest_pointer: [f32; 2],
    /// An animation value this close to its target snaps to it.
    pub settle_epsilon: f32,
}

pub const INTERACTION: Interaction = Interaction {
    hover_scale: 0.035,
    press_scale: 0.045,
    hover_boost: 0.35,
    glow_hover_boost: 0.5,
    fill_hover_lift: 0.05,
    press_dim: 0.07,
    press_dim_color: [0.0, 0.05, 0.2],
    rest_pointer: [0.5, 0.3],
    settle_epsilon: 0.004,
};

/// How glass deepens in Dark Mode (for presets with `dark_adapts`).
#[derive(Clone, Copy, Debug)]
pub struct DarkMode {
    /// Colour the body moves towards.
    pub night: [f32; 3],
    /// Alpha added to the top / bottom fill before mixing.
    pub top_alpha_add: f32,
    pub bottom_alpha_add: f32,
    /// How far the top / bottom fill moves towards `night`.
    pub top_mix: f32,
    pub bottom_mix: f32,
    /// Light is softened: scale of rim side / rim lit / glow / specular.
    pub rim_side_scale: f32,
    pub rim_lit_scale: f32,
    pub glow_scale: f32,
    pub specular_scale: f32,
}

pub const DARK: DarkMode = DarkMode {
    night: [24.0 / 255.0, 28.0 / 255.0, 38.0 / 255.0],
    top_alpha_add: 0.06,
    bottom_alpha_add: 0.08,
    top_mix: 0.45,
    bottom_mix: 0.5,
    rim_side_scale: 0.7,
    rim_lit_scale: 0.85,
    glow_scale: 0.7,
    specular_scale: 0.85,
};

// ================================================================ menu bar

/// The menu bar's wash and hairline (the strip drawn by `glass::GlassStrip`).
#[derive(Clone, Copy, Debug)]
pub struct Strip {
    /// Wash used when the bar text is dark (a bright wallpaper behind the bar).
    pub light_tint: Rgba,
    /// Wash used when the bar text is light (a dark wallpaper behind the bar).
    pub dark_tint: Rgba,
    pub hairline: Rgba,
    pub hairline_px: f32,
}

pub const STRIP: Strip = Strip {
    light_tint: [1.0, 1.0, 1.0, 0.12],
    dark_tint: [0.27, 0.29, 0.32, 0.22],
    hairline: [0.0, 0.0, 0.0, 0.06],
    hairline_px: 1.0,
};

/// The menu bar strip with the global [`TUNING`] applied.
pub fn strip() -> Strip {
    strip_with(&TUNING)
}

pub fn strip_with(t: &Tuning) -> Strip {
    let mut s = STRIP;
    for tint in [&mut s.light_tint, &mut s.dark_tint] {
        *tint = saturate(*tint, t.tint_saturation);
        tint[3] = (tint[3] * t.opacity).clamp(0.0, 1.0);
    }
    s.hairline[3] = (s.hairline[3] * t.border_opacity).clamp(0.0, 1.0);
    s.hairline_px = (s.hairline_px * t.border_width).max(0.0);
    s
}

// ============================================================ popover chrome

/// Small glass accents that GTK paints from CSS: the popover arrow, the
/// accent capsule under a hovered menu item, and the open-menu capsule in the
/// menu bar. They are generated into CSS by [`css`] so they follow the tuning.
#[derive(Clone, Copy, Debug)]
pub struct MenuChrome {
    /// Fill of the little arrow pointing from a popover at its anchor.
    pub arrow_fill: Rgba,
    /// Hovered menu item: accent gradient, top highlight, rim and drop shadow.
    pub item_hover_top: Rgba,
    pub item_hover_bottom: Rgba,
    pub item_hover_highlight: Rgba,
    pub item_hover_rim: Rgba,
    pub item_hover_shadow: Rgba,
    /// Fade of a menu item's background on hover.
    pub item_transition_ms: f32,
    pub item_easing: Easing,
    /// Menu-bar button while its menu is open.
    pub open_fill: Rgba,
    pub open_gradient_top: Rgba,
    pub open_gradient_bottom: Rgba,
    pub open_rim: Rgba,
    pub open_highlight: Rgba,
    pub open_radius: f32,
    /// Room (px) added around a shadow when CSS reserves space for it.
    pub shadow_room_slack: f32,
    /// Gap under the Dock shelf (it sits at the screen edge).
    pub dock_bottom_gap: f32,
}

pub const MENU_CHROME: MenuChrome = MenuChrome {
    arrow_fill: rgba(64, 88, 124, 0.62),
    item_hover_top: rgba(40, 150, 255, 0.96),
    item_hover_bottom: rgba(0, 104, 246, 0.96),
    item_hover_highlight: rgba(255, 255, 255, 0.42),
    item_hover_rim: rgba(190, 228, 255, 0.45),
    item_hover_shadow: rgba(0, 40, 140, 0.28),
    item_transition_ms: 120.0,
    item_easing: Easing::Exponential,
    // macOS draws the open title as a flat translucent capsule: no rim, no
    // inner highlight, no gradient.
    open_fill: rgba(255, 255, 255, 0.22),
    open_gradient_top: rgba(255, 255, 255, 0.0),
    open_gradient_bottom: rgba(255, 255, 255, 0.0),
    open_rim: rgba(255, 255, 255, 0.0),
    open_highlight: rgba(255, 255, 255, 0.0),
    open_radius: 13.0,
    shadow_room_slack: 4.0,
    dock_bottom_gap: 2.0,
};

// ====================================================================== CSS

fn css_channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn css_rgba(c: Rgba) -> String {
    format!(
        "rgba({}, {}, {}, {})",
        css_channel(c[0]),
        css_channel(c[1]),
        css_channel(c[2]),
        c[3].clamp(0.0, 1.0)
    )
}

fn css_radius(radius: Radius) -> String {
    match radius {
        Radius::Fixed(r) => format!("{r}px"),
        Radius::Capsule => "999px".to_string(),
    }
}

fn scaled(mut c: Rgba, factor: f32) -> Rgba {
    c[3] = (c[3] * factor).clamp(0.0, 1.0);
    c
}

/// CSS that has to agree with the glass painters: room for shadows, corner
/// radii, popover accents and transitions. Appended after the static
/// stylesheet in `style::install`, so the static CSS carries none of these
/// values and cannot drift from the configuration.
pub fn css() -> String {
    css_with(&TUNING)
}

pub fn css_with(t: &Tuning) -> String {
    let c = MENU_CHROME;
    let dock = Preset::dock().tuned_with(t);
    let menu = Preset::menu().tuned_with(t);
    let bubble = Preset::bubble().tuned_with(t);

    // The Dock window must be large enough for its shadow, or it is clipped.
    let dock_layer = dock.shadow.ambient;
    let dock_room =
        (dock_layer.blur + dock_layer.spread.max(0.0) + dock_layer.dx.abs() + c.shadow_room_slack).ceil();

    // Popovers reserve room for the largest shadow of any glass shown in one.
    let (mut dy, mut blur, mut spread) = (0.0f32, 0.0f32, 0.0f32);
    for preset in [Preset::menu(), Preset::clear(), Preset::accent(), Preset::bubble()] {
        let layer = preset.tuned_with(t).shadow.ambient;
        dy = dy.max(layer.dy);
        blur = blur.max(layer.blur);
        spread = spread.max(layer.spread);
    }

    let item_ms = c.item_transition_ms / t.animation_speed.max(0.01);
    let item_easing = t.easing.unwrap_or(c.item_easing).css();
    let opacity = t.opacity;

    let mut out = String::new();
    out.push_str("\n/* ---- Generated from glass_config.rs: do not edit here. ---- */\n");
    out.push_str(&format!(
        ".dock-glass {{ margin: {dock_room}px {dock_room}px {gap}px {dock_room}px; }}\n\
         .dock {{ border-radius: {dock_radius}; }}\n",
        gap = c.dock_bottom_gap,
        dock_radius = css_radius(dock.radius),
    ));
    out.push_str(&format!(
        "popover.lunar-menu-popover {{ border-radius: {menu_radius}; }}\n\
         popover.lunar-menu-popover > contents {{ box-shadow: 0 {dy}px {blur}px {spread}px rgba(0, 0, 0, 0.001); }}\n\
         .lunar-system-menu {{ border-radius: {menu_radius}; }}\n\
         .lunar-system-menu.dock-label {{ border-radius: {bubble_radius}; }}\n",
        menu_radius = css_radius(menu.radius),
        bubble_radius = css_radius(bubble.radius),
    ));
    out.push_str(&format!(
        "popover.lunar-menu-popover.dock-context-popover > arrow,\n\
         popover.lunar-menu-popover.dock-label-popover > arrow {{ background-color: {arrow}; }}\n",
        arrow = css_rgba(scaled(c.arrow_fill, opacity)),
    ));
    // Hovered / pressed menu items are a flat accent fill (no gradient, rim or
    // glass shadow), like the selected row in Settings.
    out.push_str(&format!(
        "button.lunar-menu-item {{ transition: background-color {item_ms}ms {item_easing}, color {item_ms}ms {item_easing}; }}\n\
         button.lunar-menu-item:hover,\n\
         button.lunar-menu-item:active {{\n\
         \x20   background-color: #0a84ff;\n\
         \x20   background-image: none;\n\
         \x20   box-shadow: none;\n\
         }}\n",
    ));
    out.push_str(&format!(
        "menubutton.lunar-menu-button.menu-open > button {{\n\
         \x20   background-color: {fill};\n\
         \x20   background-image: linear-gradient(to bottom, {g_top}, {g_bottom});\n\
         \x20   box-shadow: inset 0 0 0 0.5px {rim}, inset 0 1px 0 {highlight};\n\
         \x20   border-radius: {radius}px;\n\
         }}\n",
        fill = css_rgba(scaled(c.open_fill, opacity)),
        g_top = css_rgba(scaled(c.open_gradient_top, opacity)),
        g_bottom = css_rgba(scaled(c.open_gradient_bottom, opacity)),
        rim = css_rgba(scaled(c.open_rim, t.border_opacity)),
        highlight = css_rgba(scaled(c.open_highlight, t.border_opacity)),
        radius = c.open_radius * t.corner_radius,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn all() -> [Preset; 6] {
        [
            Preset::clear(),
            Preset::accent(),
            Preset::menu(),
            Preset::dock(),
            Preset::bubble(),
            Preset::sidebar(),
        ]
    }

    #[test]
    fn neutral_tuning_leaves_every_preset_unchanged() {
        for p in all() {
            let t = p.tuned_with(&NEUTRAL);
            assert!(close(t.body.top[3], p.body.top[3]));
            assert!(close(t.body.bottom[0], p.body.bottom[0]));
            assert!(close(t.refraction.glow[3], p.refraction.glow[3]));
            assert!(close(t.refraction.reach, p.refraction.reach));
            assert!(close(t.highlight.strength, p.highlight.strength));
            assert!(close(t.highlight.key_distance, p.highlight.key_distance));
            assert!(close(t.shadow.ambient.blur, p.shadow.ambient.blur));
            assert!(close(t.shadow.ambient.dy, p.shadow.ambient.dy));
            assert!(close(t.motion.hover_in_ms, p.motion.hover_in_ms));
            assert_eq!(t.radius, p.radius);
        }
    }

    #[test]
    fn tuning_reaches_every_preset() {
        let t = Tuning { opacity: 0.5, corner_radius: 2.0, shadow_softness: 2.0, ..NEUTRAL };
        for p in all() {
            let tuned = p.tuned_with(&t);
            assert!(close(tuned.body.top[3], p.body.top[3] * 0.5));
            assert!(close(tuned.shadow.ambient.blur, p.shadow.ambient.blur * 2.0));
            if let (Radius::Fixed(a), Radius::Fixed(b)) = (p.radius, tuned.radius) {
                assert!(close(b, a * 2.0));
            }
        }
    }

    #[test]
    fn default_light_matches_the_original_offsets() {
        let h = Highlight::DEFAULT;
        let a = h.azimuth.to_radians();
        let (dx, dy) = (-a.sin() * h.key_distance, a.cos() * h.key_distance);
        assert!((dx - 1.1).abs() < 0.05 && (dy - 1.3).abs() < 0.05);
    }

    #[test]
    fn easings_start_at_zero_and_end_at_one() {
        for e in [
            Easing::Exponential,
            Easing::Linear,
            Easing::EaseOutCubic,
            Easing::EaseInOutCubic,
            Easing::EaseOutQuint,
            Easing::EaseOutBack,
        ] {
            assert!(close(e.apply(0.0), 0.0));
            assert!(close(e.apply(1.0), 1.0));
        }
    }

    #[test]
    fn default_css_keeps_the_shipped_geometry() {
        let css = css_with(&NEUTRAL);
        assert!(css.contains("margin: 30px 30px 2px 30px"));
        assert!(css.contains("0 9px 26px 0px rgba(0, 0, 0, 0.001)"));
        assert!(css.contains("border-radius: 26px"));
        assert!(css.contains("transition: background-color 120ms ease-out"));
    }

    #[test]
    fn blur_regions_follow_the_painted_shape() {
        assert_eq!(Preset::dock().blur_corner(), 26);
        assert_eq!(Preset::menu().blur_corner(), 12);
        assert_eq!(Preset::clear_capsule().blur_corner(), CAPSULE_BLUR_RADIUS);
    }
}
