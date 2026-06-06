//! Bounded raster prep for `ratatui-image` and Kitty album art.
//!
//! Covers are **contain**-fit into the widget cell rect (aspect-aware using font pixel size).
//! The draw rect is shrunk via [`contain_fit_rect_in_cells`] so gutters are not painted; bitmaps
//! are scaled with [`prepare_art_image_for_rect_contain_fit`] without a letterbox canvas.

use image::{imageops::FilterType, DynamicImage};
use ratatui::layout::Rect;
use ratatui_image::FontSize;

/// Cap per edge when scaling bitmaps (avoid huge protocol payloads).
pub const MAX_ART_EDGE_PX: u32 = 1024;

/// Pixel size of `inner` in terminal pixels, capped per edge.
pub fn pixel_budget_for_rect(inner: Rect, font: FontSize) -> (u32, u32) {
    let w = (inner.width as u32 * font.0 as u32).min(MAX_ART_EDGE_PX);
    let h = (inner.height as u32 * font.1 as u32).min(MAX_ART_EDGE_PX);
    (w.max(1), h.max(1))
}

/// Scale (up or down) so the image fits inside `max_w × max_h` while preserving aspect ratio.
pub fn fit_image_to_pixel_budget(img: DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 {
        return img;
    }
    let (tw, th) = fit_inside(iw, ih, max_w, max_h);
    if (tw, th) == (iw, ih) {
        return img;
    }
    img.resize_exact(tw, th, FilterType::Triangle)
}

fn fit_inside(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let wratio = max_w as f64 / w as f64;
    let hratio = max_h as f64 / h as f64;
    let ratio = f64::min(wratio, hratio);
    let nw = ((w as f64 * ratio).round() as u32).max(1);
    let nh = ((h as f64 * ratio).round() as u32).max(1);
    (nw, nh)
}

/// Terminal-cell [`Rect`] inside `inner` that **contain**-fits `img`, centered (integer cols/rows).
///
/// Uses `font` so aspect ratio matches terminal **pixels** (cells are rarely square in px).
/// Used so album art is drawn only in the cells the cover occupies — gutters stay unpainted.
pub fn contain_fit_rect_in_cells(img: &DynamicImage, inner: Rect, font: FontSize) -> Rect {
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 || inner.width == 0 || inner.height == 0 {
        return inner;
    }
    let fw = font.0 as u32;
    let fh = font.1 as u32;
    if fw == 0 || fh == 0 {
        return inner;
    }
    let max_w_px = inner.width as u32 * fw;
    let max_h_px = inner.height as u32 * fh;
    let (fit_w_px, fit_h_px) = fit_inside(iw, ih, max_w_px, max_h_px);

    let w = fit_w_px.div_ceil(fw).max(1).min(inner.width as u32) as u16;
    let h = fit_h_px.div_ceil(fh).max(1).min(inner.height as u32) as u16;

    let x = inner.x + (inner.width.saturating_sub(w)) / 2;
    let y = inner.y + (inner.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// Contain-fit into the pixel budget for `rect` — no letterbox canvas (bitmap matches fitted size).
pub fn prepare_art_image_for_rect_contain_fit(
    img: DynamicImage,
    rect: Rect,
    font: FontSize,
) -> DynamicImage {
    let (max_w, max_h) = pixel_budget_for_rect(rect, font);
    fit_image_to_pixel_budget(img, max_w, max_h)
}

/// FNV-1a 64-bit digest of raw image bytes.
///
/// Used for Now Playing cache keys so consecutive tracks with different `cover_id` but identical
/// pixels do not trigger re-encode / re-transmit.
pub fn art_bytes_fingerprint(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn solid(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::new(w, h))
    }

    #[test]
    fn contain_fit_rect_square_in_wide_panel() {
        let inner = Rect::new(0, 0, 20, 10);
        let img = solid(500, 500);
        let font = (10u16, 10u16);
        let fit = contain_fit_rect_in_cells(&img, inner, font);
        assert_eq!(fit.width, 10);
        assert_eq!(fit.height, 10);
        assert_eq!(fit.x, 5);
        assert_eq!(fit.y, 0);
    }

    #[test]
    fn contain_fit_rect_wide_image_in_tall_panel() {
        let inner = Rect::new(0, 0, 10, 20);
        let img = solid(800, 400);
        let font = (10u16, 10u16);
        let fit = contain_fit_rect_in_cells(&img, inner, font);
        assert_eq!(fit.width, 10);
        assert_eq!(fit.height, 5);
        assert_eq!(fit.x, 0);
        assert_eq!(fit.y, 7);
    }

    #[test]
    fn contain_fit_rect_square_fills_tall_cells_when_px_aspect_matches() {
        // 20×10 cells at 10×20 px/cell → 200×200 px panel; square cover fills it.
        let inner = Rect::new(0, 0, 20, 10);
        let img = solid(500, 500);
        let font = (10u16, 20u16);
        let fit = contain_fit_rect_in_cells(&img, inner, font);
        assert_eq!(fit, inner);
    }
}
