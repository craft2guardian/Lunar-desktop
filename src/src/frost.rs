//! Renders the menu bar's frosted-glass backdrop on the CPU.
//!
//! Wayland has no portable "blur what is behind me" request, but the menu bar
//! only ever sits directly on the wallpaper, so we can blur the wallpaper
//! itself. We crop it exactly as the wallpaper window shows it (same cover
//! maths) and blur it with edge clamping, so the strip neither fades out nor
//! shifts at the screen edges (which GPU blur nodes would do).
//!
//! This runs once per output size / scale change over a strip a few dozen
//! pixels tall, so the cost is negligible and nothing runs while idle.

use crate::wallpaper::{cover_rect, Image};

/// Box-blur passes per axis. Three approximate a Gaussian closely.
const PASSES: usize = 3;

/// Renders the top `height` pixel rows of the wallpaper as shown on an output
/// of `output` logical size at integer `scale`, blurred with a box radius of
/// `radius` pixels per pass. Returns `width * height * 4` bytes of opaque RGBA.
///
/// Rows near the bottom of the strip are only blur context: callers should
/// include `PASSES * radius` extra rows below what they intend to show.
pub fn render(
    image: &Image,
    output: (f32, f32),
    width: usize,
    height: usize,
    scale: usize,
    radius: usize,
) -> Vec<u8> {
    let cover = cover_rect(image.width as f32, image.height as f32, output.0, output.1);
    let k = scale as f32;
    // Source pixels per logical pixel on each axis.
    let (sx, sy) = (
        image.width as f32 / cover.width,
        image.height as f32 / cover.height,
    );

    // Temporary full-size copy; freed when this function returns.
    let (pixels, stride) = image.download_rgba();
    let source = Source {
        data: &pixels,
        stride,
        width: image.width,
        height: image.height,
    };

    let mut buf = vec![0f32; width * height * 3];
    for y in 0..height {
        let ty = ((y as f32 + 0.5) / k - cover.y) * sy - 0.5;
        for x in 0..width {
            let tx = ((x as f32 + 0.5) / k - cover.x) * sx - 0.5;
            let rgb = source.sample(tx, ty);
            buf[(y * width + x) * 3..][..3].copy_from_slice(&rgb);
        }
    }
    drop(pixels);

    blur(&mut buf, width, height, radius);

    let mut out = Vec::with_capacity(width * height * 4);
    for px in buf.chunks_exact(3) {
        out.extend(px.iter().map(|c| (c + 0.5).clamp(0.0, 255.0) as u8));
        out.push(255);
    }
    out
}

/// Borrowed RGBA8 pixels of the wallpaper.
struct Source<'a> {
    data: &'a [u8],
    stride: usize,
    width: usize,
    height: usize,
}

impl Source<'_> {
    /// Bilinear sample with clamp-to-edge. Coordinates are in source pixels.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let max_x = (self.width - 1) as f32;
        let max_y = (self.height - 1) as f32;
        let (x, y) = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);

        let at = |px: usize, py: usize, c: usize| self.data[py * self.stride + px * 4 + c] as f32;
        let mut rgb = [0.0; 3];
        for (c, out) in rgb.iter_mut().enumerate() {
            let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
            let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
            *out = top * (1.0 - fy) + bottom * fy;
        }
        rgb
    }
}

/// Separable box blur on interleaved RGB, repeated `PASSES` times per axis.
fn blur(buf: &mut [f32], width: usize, height: usize, radius: usize) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let mut scratch = Vec::with_capacity(width.max(height));
    for _ in 0..PASSES {
        for y in 0..height {
            for c in 0..3 {
                blur_line(buf, (y * width) * 3 + c, 3, width, radius, &mut scratch);
            }
        }
        for x in 0..width {
            for c in 0..3 {
                blur_line(buf, x * 3 + c, width * 3, height, radius, &mut scratch);
            }
        }
    }
}

/// Box-blurs one line of `len` samples (`step` apart, starting at `base`)
/// in place with a sliding window and clamp-to-edge.
fn blur_line(buf: &mut [f32], base: usize, step: usize, len: usize, r: usize, line: &mut Vec<f32>) {
    line.clear();
    line.extend((0..len).map(|i| buf[base + i * step]));

    let last = len as isize - 1;
    let at = |i: isize| line[i.clamp(0, last) as usize];
    let r = r as isize;
    let window = (2 * r + 1) as f32;

    let mut sum: f32 = (-r..=r).map(at).sum();
    for i in 0..len as isize {
        buf[base + i as usize * step] = sum / window;
        sum += at(i + r + 1) - at(i - r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_flat_colour_flat() {
        let mut buf = vec![100.0; 40 * 12 * 3];
        blur(&mut buf, 40, 12, 5);
        assert!(buf.iter().all(|v| (v - 100.0).abs() < 1e-3));
    }

    #[test]
    fn blur_smooths_a_step_and_preserves_the_edges() {
        // One row, left half black, right half white.
        let (w, h) = (64, 1);
        let mut buf = vec![0.0; w * h * 3];
        for x in w / 2..w {
            buf[x * 3..x * 3 + 3].fill(255.0);
        }
        blur(&mut buf, w, h, 4);
        let first = buf[0];
        let last = buf[(w - 1) * 3];
        assert!(first < 1.0, "left edge should stay dark, got {first}");
        assert!(last > 254.0, "right edge should stay bright, got {last}");
        let mid = buf[(w / 2) * 3];
        assert!(
            mid > 100.0 && mid < 160.0,
            "step centre should be ~half, got {mid}"
        );
    }
}
