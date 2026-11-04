//! Dock magnification maths (no GTK here, so it is unit-testable).
//!
//! Everything is expressed in *rest space*: the coordinates the Dock content
//! has when nothing is magnified, measured from its left edge. The pointer is
//! converted into rest space once, and each icon's scale then depends only on
//! how far the pointer is from that icon's resting centre. Because the Dock
//! window stays centred on the screen, nothing here feeds back on itself.

/// Gap between neighbouring tiles in the icon row.
pub const ROW_SPACING: f64 = 3.0;
/// Gap between the app row, resize handle, minimized row and Trash.
pub const CONTENT_SPACING: f64 = 2.0;
/// Width of the resize handle (the separator) between apps and Trash.
pub const HANDLE_WIDTH: f64 = 12.0;

/// Height of the resize handle; the Dock is never shorter than this.
pub const HANDLE_HEIGHT: f64 = 34.0;

/// Width of the bell curve, in icon widths. Larger values magnify more
/// neighbours: about 2 icons either side get a visible boost, like macOS.
const SIGMA_IN_ICONS: f64 = 1.1;

/// The largest scale for a "Magnification" preference of 0-100 (0 = off).
pub fn max_scale_for(preference: f64) -> f64 {
    1.0 + preference.clamp(0.0, 100.0) / 100.0
}

/// Scale of an icon whose resting centre is `distance` px from the pointer.
/// `amount` (0..=1) fades the whole effect in and out.
pub fn scale_at(distance: f64, size: f64, amount: f64, max_scale: f64) -> f64 {
    let sigma = (size * SIGMA_IN_ICONS).max(1.0);
    let bell = (-(distance * distance) / (2.0 * sigma * sigma)).exp();
    1.0 + (max_scale - 1.0).max(0.0) * amount.clamp(0.0, 1.0) * bell
}

/// Where everything sits when the Dock is at rest.
#[derive(Debug, Clone, PartialEq)]
pub struct RestLayout {
    /// Centre of each visible app tile, in order.
    pub tile_centers: Vec<f64>,
    /// Centre of each visible minimized-window tile, in order.
    pub minimized_centers: Vec<f64>,
    /// Centre of the Trash.
    pub trash_center: f64,
    /// Width of the whole content (tiles, handle, minimized tiles and Trash).
    pub width: f64,
}

/// Lays out `tiles` visible app tiles of `size` px, then the handle, then Trash.
pub fn rest_layout(tiles: usize, size: f64) -> RestLayout {
    rest_layout_with_minimized(tiles, size, &[])
}

/// Like `rest_layout`, but accounts for the optional minimized-window row.
/// The widths are the resting (unmagnified) widths of its visible tiles.
pub fn rest_layout_with_minimized(
    tiles: usize,
    size: f64,
    minimized_widths: &[f64],
) -> RestLayout {
    let mut x = 0.0;
    let mut tile_centers = Vec::with_capacity(tiles);
    for _ in 0..tiles {
        tile_centers.push(x + size / 2.0);
        x += size + ROW_SPACING;
    }
    if tiles > 0 {
        // No gap after the last tile (the Box only puts gaps *between* children).
        x -= ROW_SPACING;
    }

    // The app row is always a child of the content box, even when empty.
    x += CONTENT_SPACING;
    x += HANDLE_WIDTH;

    let mut minimized_centers = Vec::with_capacity(minimized_widths.len());
    if !minimized_widths.is_empty() {
        x += CONTENT_SPACING;
        for (index, width) in minimized_widths.iter().copied().enumerate() {
            minimized_centers.push(x + width / 2.0);
            x += width;
            if index + 1 < minimized_widths.len() {
                x += ROW_SPACING;
            }
        }
    }
    // There is always a gap between the last visible child and the Trash.
    x += CONTENT_SPACING;
    let trash_center = x + size / 2.0;
    x += size;

    RestLayout {
        tile_centers,
        minimized_centers,
        trash_center,
        width: x,
    }
}

/// Moves `current` towards `target`, frame-rate independently. `tau` is the
/// time constant in seconds: after `tau` about 63 % of the gap is closed.
pub fn approach(current: f64, target: f64, dt: f64, tau: f64) -> f64 {
    if tau <= 0.0 {
        return target;
    }
    current + (target - current) * (1.0 - (-dt / tau).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_peaks_under_the_pointer_and_fades_with_distance() {
        let size = 58.0;
        let peak = scale_at(0.0, size, 1.0, 1.8);
        let near = scale_at(size, size, 1.0, 1.8);
        let far = scale_at(size * 4.0, size, 1.0, 1.8);
        assert!((peak - 1.8).abs() < 1e-9);
        assert!(near < peak && near > 1.3);
        assert!(far < 1.01);
    }

    #[test]
    fn amount_zero_means_no_magnification() {
        assert_eq!(scale_at(0.0, 58.0, 0.0, 2.0), 1.0);
    }

    #[test]
    fn preference_zero_is_off() {
        assert_eq!(max_scale_for(0.0), 1.0);
        assert_eq!(scale_at(0.0, 58.0, 1.0, max_scale_for(0.0)), 1.0);
        assert_eq!(max_scale_for(100.0), 2.0);
    }

    #[test]
    fn rest_layout_matches_the_box_spacing() {
        let layout = rest_layout(3, 50.0);
        assert_eq!(layout.tile_centers, vec![25.0, 78.0, 131.0]);
        // 3 tiles + 2 gaps = 156, +2, handle 12, +2, trash 50.
        assert_eq!(layout.width, 156.0 + 2.0 + 12.0 + 2.0 + 50.0);
        assert_eq!(layout.trash_center, 156.0 + 2.0 + 12.0 + 2.0 + 25.0);
    }

    #[test]
    fn rest_layout_without_tiles_is_just_handle_and_trash() {
        let layout = rest_layout(0, 50.0);
        assert!(layout.tile_centers.is_empty());
        assert_eq!(layout.width, 2.0 + 12.0 + 2.0 + 50.0);
    }

    #[test]
    fn rest_layout_places_minimized_tiles_between_handle_and_trash() {
        let layout = rest_layout_with_minimized(2, 50.0, &[48.0, 72.0]);
        assert_eq!(layout.tile_centers, vec![25.0, 78.0]);
        assert_eq!(layout.minimized_centers, vec![143.0, 206.0]);
        assert_eq!(layout.trash_center, 269.0);
        assert_eq!(layout.width, 294.0);
    }

    #[test]
    fn approach_converges_without_overshoot() {
        let mut value = 0.0;
        for _ in 0..200 {
            value = approach(value, 1.0, 1.0 / 60.0, 0.05);
            assert!(value <= 1.0);
        }
        assert!(value > 0.999);
    }
}
