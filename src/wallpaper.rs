//! The desktop background: one `background`-layer surface per output, filled
//! edge-to-edge with the wallpaper using "cover" scaling (aspect ratio kept,
//! overflow cropped evenly on both sides).

use crate::context_menu::ContextMenu;
use crate::kde_shell::{self, Role};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::path::{Path, PathBuf};

/// Bundled wallpaper. Regenerate it with `scripts/generate_wallpaper.py`.
pub static WALLPAPER_JPEG: &[u8] = include_bytes!("../assets/wallpaper.jpg");

/// A decoded wallpaper, shared by every window.
pub struct Image {
    pub texture: gdk::Texture,
    /// Small luminance copy of the picture, built on first use.
    thumb: std::cell::OnceCell<Option<Thumb>>,
}

/// Which colour of text reads best on the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarTone {
    /// Dark wallpaper behind the bar: white text.
    LightText,
    /// Bright wallpaper behind the bar: near-black text.
    DarkText,
}

/// Relative luminance (linear light, 0..1) of a downscaled wallpaper.
struct Thumb {
    width: usize,
    height: usize,
    luminance: Vec<f32>,
}

impl Thumb {
    fn build(texture: &gdk::Texture) -> Option<Self> {
        let (w, h) = (texture.width().max(0) as usize, texture.height().max(0) as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let mut downloader = gdk::TextureDownloader::new(texture);
        downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = downloader.download_bytes();
        let data: &[u8] = &bytes;
        if data.len() < stride * h {
            return None;
        }

        // sRGB byte -> linear light.
        let linear: Vec<f32> = (0..256)
            .map(|value| {
                let c = value as f32 / 255.0;
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            })
            .collect();

        let tw = w.min(512);
        let th = (h * tw / w).max(1);
        let mut luminance = Vec::with_capacity(tw * th);
        for ty in 0..th {
            let y0 = ty * h / th;
            let y1 = ((ty + 1) * h / th).clamp(y0 + 1, h);
            let step_y = ((y1 - y0) / 3).max(1);
            for tx in 0..tw {
                let x0 = tx * w / tw;
                let x1 = ((tx + 1) * w / tw).clamp(x0 + 1, w);
                let step_x = ((x1 - x0) / 3).max(1);
                let (mut sum, mut count) = (0.0f32, 0.0f32);
                let mut y = y0;
                while y < y1 {
                    let mut x = x0;
                    while x < x1 {
                        let i = y * stride + x * 4;
                        sum += 0.2126 * linear[data[i] as usize]
                            + 0.7152 * linear[data[i + 1] as usize]
                            + 0.0722 * linear[data[i + 2] as usize];
                        count += 1.0;
                        x += step_x;
                    }
                    y += step_y;
                }
                luminance.push(sum / count);
            }
        }
        Some(Self {
            width: tw,
            height: th,
            luminance,
        })
    }

    /// Mean luminance of the part of the picture inside the fractional
    /// rectangle `[u0, u1] x [v0, v1]` (0..1 across and down the picture).
    fn mean(&self, u0: f32, u1: f32, v0: f32, v1: f32) -> f32 {
        let to_range = |a: f32, b: f32, size: usize| {
            let first = ((a.clamp(0.0, 1.0) * size as f32).floor() as usize).min(size - 1);
            let last = ((b.clamp(0.0, 1.0) * size as f32).ceil() as usize).clamp(first + 1, size);
            (first, last)
        };
        let (x0, x1) = to_range(u0, u1, self.width);
        let (y0, y1) = to_range(v0, v1, self.height);
        let mut sum = 0.0;
        for y in y0..y1 {
            sum += self.luminance[y * self.width + x0..y * self.width + x1]
                .iter()
                .sum::<f32>();
        }
        sum / ((x1 - x0) * (y1 - y0)) as f32
    }
}

impl Image {
    pub fn load_bundled() -> Result<Self, glib::Error> {
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from_static(WALLPAPER_JPEG))?;
        Ok(Self::from_texture(texture))
    }

    fn from_texture(texture: gdk::Texture) -> Self {
        Self {
            texture,
            thumb: std::cell::OnceCell::new(),
        }
    }

    pub fn load_file(path: &Path) -> Result<Self, glib::Error> {
        Ok(Self::from_texture(gdk::Texture::from_filename(path)?))
    }

    /// The wallpaper chosen in Settings, or the bundled one when none is
    /// chosen or the chosen file can no longer be read.
    pub fn load_current() -> Result<Self, glib::Error> {
        if let Some(path) = crate::preferences::wallpaper_path() {
            match Self::load_file(&path) {
                Ok(image) => return Ok(image),
                Err(error) => eprintln!(
                    "sonoma-desktop: could not load wallpaper {}: {error}",
                    path.display()
                ),
            }
        }
        Self::load_bundled()
    }
}

impl Image {
    /// Picks the menu bar text colour for the top `bar_height` pixels of an
    /// output of `output` logical size showing this wallpaper (cover-scaled,
    /// exactly as the wallpaper window draws it).
    ///
    /// The bar is split into segments; for each candidate (white text over the
    /// dark wash, near-black text over the light wash) the WCAG contrast
    /// against the washed-out wallpaper is averaged, and the better one wins.
    /// Contrast per segment is capped so one extreme patch cannot decide it.
    pub fn bar_tone(&self, output: (f32, f32), bar_height: f32) -> BarTone {
        use crate::glass::{DARK_TINT_ALPHA, LIGHT_TINT_ALPHA};
        const SEGMENTS: usize = 24;
        const CONTRAST_CAP: f32 = 10.0;
        // Luminance of the two washes (white; the dark grey in glass.rs) and
        // of the near-black text colour.
        const LIGHT_WASH: f32 = 1.0;
        const DARK_WASH: f32 = 0.06;
        const DARK_TEXT: f32 = 0.006;

        let fallback = BarTone::LightText;
        let Some(thumb) = self.thumb.get_or_init(|| Thumb::build(&self.texture)).as_ref() else {
            return fallback;
        };
        let (output_w, output_h) = output;
        let (image_w, image_h) = (self.texture.width() as f32, self.texture.height() as f32);
        if output_w <= 0.0 || output_h <= 0.0 || image_w <= 0.0 || image_h <= 0.0 {
            return fallback;
        }

        // Cover maths: the picture is scaled to fill the output and centred.
        let scale = (output_w / image_w).max(output_h / image_h);
        let (shown_w, shown_h) = (image_w * scale, image_h * scale);
        let (offset_x, offset_y) = ((output_w - shown_w) / 2.0, (output_h - shown_h) / 2.0);
        let (u_start, u_end) = (-offset_x / shown_w, (output_w - offset_x) / shown_w);
        let (v_start, v_end) = (-offset_y / shown_h, (bar_height - offset_y) / shown_h);

        let (mut dark_text, mut light_text) = (0.0f32, 0.0f32);
        for segment in 0..SEGMENTS {
            let a = u_start + (u_end - u_start) * segment as f32 / SEGMENTS as f32;
            let b = u_start + (u_end - u_start) * (segment + 1) as f32 / SEGMENTS as f32;
            let y = thumb.mean(a, b, v_start, v_end);

            let light_bg = y * (1.0 - LIGHT_TINT_ALPHA) + LIGHT_WASH * LIGHT_TINT_ALPHA;
            dark_text += ((light_bg + 0.05) / (DARK_TEXT + 0.05)).min(CONTRAST_CAP);
            let dark_bg = y * (1.0 - DARK_TINT_ALPHA) + DARK_WASH * DARK_TINT_ALPHA;
            light_text += (1.05 / (dark_bg + 0.05)).min(CONTRAST_CAP);
        }
        if dark_text > light_text {
            BarTone::DarkText
        } else {
            BarTone::LightText
        }
    }
}

/// Shows `image` in an existing wallpaper window.
pub fn set_image(window: &gtk::Window, image: &Image) {
    if let Some(picture) = window.child().and_downcast::<gtk::Picture>() {
        picture.set_paintable(Some(&image.texture));
    }
}

const IMAGE_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"];

/// Where wallpapers added through Settings are kept.
pub fn library_dir() -> Option<PathBuf> {
    crate::preferences::config_dir().map(|directory| directory.join("wallpapers"))
}

/// The wallpapers added through Settings, newest first.
pub fn library() -> Vec<PathBuf> {
    let Some(entries) = library_dir().and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, path))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().map(|(_, path)| path).collect()
}

/// Checks that `source` is a readable image and copies it into the library, so
/// the wallpaper keeps working if the original is moved or deleted.
pub fn import(source: &Path) -> Result<PathBuf, String> {
    gdk::Texture::from_filename(source).map_err(|error| format!("not a usable image: {error}"))?;
    let directory = library_dir().ok_or("could not determine the configuration directory")?;
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    if source.parent() == Some(directory.as_path()) {
        return Ok(source.to_path_buf());
    }
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("wallpaper");
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("jpg");
    let mut target = directory.join(format!("{stem}.{extension}"));
    let mut number = 2;
    while target.exists() {
        target = directory.join(format!("{stem}-{number}.{extension}"));
        number += 1;
    }
    std::fs::copy(source, &target).map_err(|error| error.to_string())?;
    Ok(target)
}

/// Creates (but does not present) the wallpaper window for `monitor`.
pub fn window(app: &gtk::Application, image: &Image, monitor: &gdk::Monitor) -> gtk::Window {
    let picture = gtk::Picture::for_paintable(&image.texture);
    picture.set_content_fit(gtk::ContentFit::Cover);
    // Without this the picture would insist on the texture's full pixel size.
    picture.set_can_shrink(true);

    let window = gtk::Window::builder()
        .application(app)
        .title("Sonoma Wallpaper")
        .decorated(false)
        .css_classes(["sonoma-wallpaper"])
        .child(&picture)
        .build();

    // Right-clicking the desktop opens a small context menu.
    let menu = ContextMenu::new(gtk::PositionType::Bottom, &["desktop-context-menu"]);
    menu.set_parent(&picture);
    {
        let app = app.clone();
        menu.append_action("Change Wallpaper\u{2026}", true, move || {
            crate::settings::show_page(&app, "wallpaper");
        });
    }
    let right_click = gtk::GestureClick::new();
    right_click.set_button(gdk::BUTTON_SECONDARY);
    {
        let menu = menu.clone();
        right_click.connect_pressed(move |_, _, x, y| menu.popup_at(x as i32, y as i32));
    }
    picture.add_controller(right_click);
    picture.connect_destroy(move |_| menu.unparent());

    let geometry = monitor.geometry();
    window.set_default_size(geometry.width(), geometry.height());
    kde_shell::configure_window(&window, monitor, Role::Desktop, geometry.x(), geometry.y());
    crate::preferences::apply_dark_mode(&window);
    window
}
