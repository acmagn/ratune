//! Bounded raster prep for `ratatui-image`.
//!
//! `Resize::Scale` in ratatui-image pads the **full** cell×font pixel rectangle; if the source
//! aspect ratio does not match that rectangle, ratatui-image fills the rest with the pad colour
//! (“black bands”). We **center-crop** the cover to the cell aspect ratio first, then scale down
//! inside the 1024 px budget, so the bitmap matches the widget area more closely.

use image::{DynamicImage, ImageBuffer, Rgba, RgbaImage, imageops::{self, FilterType}};
use ratatui::layout::Rect;
use ratatui_image::FontSize;

/// Same cap as `kitty_art::render_image` (avoid huge protocol payloads).
pub const MAX_ART_EDGE_PX: u32 = 1024;

/// Guard for full-cell×font Sixel bitmaps (`inner × font`); above this, fall back to capped rect prep.
pub const MAX_SIXEL_PREP_PIXELS: u128 = 12_000_000;

/// Home Recently Played strip: encode covers at this multiple of the on-screen pixel budget.
/// The widget still occupies the same `Rect` in cells; ratatui-image scales down for display,
/// but Sixel / halfblocks look much less mushy than fitting straight to `cols×font × rows×font`.
pub const STRIP_ENCODE_SUPERRES: u32 = 2;

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

/// Center-crop `img` to match the aspect ratio of `rect` in terminal pixels (`rect × font`).
pub fn crop_center_to_cell_aspect(img: DynamicImage, rect: Rect, font: FontSize) -> DynamicImage {
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 || rect.width == 0 || rect.height == 0 {
        return img;
    }
    let cell_w_px = rect.width as u32 * font.0 as u32;
    let cell_h_px = rect.height as u32 * font.1 as u32;
    if cell_w_px == 0 || cell_h_px == 0 {
        return img;
    }
    let tr = cell_w_px as f64 / cell_h_px as f64;
    let ir = iw as f64 / ih as f64;
    if (ir - tr).abs() < 1e-4 {
        return img;
    }
    let (crop_w, crop_h) = if ir > tr {
        let crop_w = (ih as f64 * tr).round() as u32;
        (crop_w.min(iw).max(1), ih)
    } else {
        let crop_h = (iw as f64 / tr).round() as u32;
        (iw, crop_h.min(ih).max(1))
    };
    let x = iw.saturating_sub(crop_w) / 2;
    let y = ih.saturating_sub(crop_h) / 2;
    img.crop_imm(x, y, crop_w, crop_h)
}

/// Center-crop to the cell aspect ratio, then scale into the pixel budget (≤1024 per edge).
pub fn prepare_art_image_for_rect(img: DynamicImage, rect: Rect, font: FontSize) -> DynamicImage {
    let img = crop_center_to_cell_aspect(img, rect, font);
    let (max_w, max_h) = pixel_budget_for_rect(rect, font);
    fit_image_to_pixel_budget(img, max_w, max_h)
}

/// Uniformly scale `w×h` down so `max(w,h) ≤ max_edge` (preserves aspect). No-op if already smaller.
pub fn uniform_scale_dimensions_to_max_edge(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    if w <= max_edge && h <= max_edge {
        return (w, h);
    }
    let s = max_edge as f64 / w.max(h) as f64;
    let nw = ((w as f64) * s).round() as u32;
    let nh = ((h as f64) * s).round() as u32;
    (nw.max(1), nh.max(1))
}

/// Sixel path uses `DynamicImage::to_rgb8()` in ratatui-image, which **drops alpha as black**.
/// Some PNG covers have transparency; compositing onto `bg` makes letterboxing match the panel.
fn flatten_rgba_onto_background(img: DynamicImage, bg: Rgba<u8>) -> DynamicImage {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 {
        return DynamicImage::ImageRgba8(rgba);
    }
    let mut canvas: RgbaImage = ImageBuffer::from_pixel(w, h, bg);
    imageops::overlay(&mut canvas, &rgba, 0, 0);
    DynamicImage::ImageRgba8(canvas)
}

/// Contain-fit into `target_w×target_h`, then pad to that exact size (centered).
///
/// Used when the bitmap must match a physical pixel size exactly (Kitty placement aspect, or
/// ratatui-image Sixel `desired` cells). **Do not** use `image::resize(w,h)` alone — it stretches.
///
/// Output is composited onto `pad` so alpha is never passed through to Sixel as spurious black.
pub fn prepare_art_image_for_exact_pixels_contain_centered(
    img: DynamicImage,
    target_w: u32,
    target_h: u32,
    pad: Rgba<u8>,
) -> DynamicImage {
    let target_w = target_w.max(1);
    let target_h = target_h.max(1);
    let fitted = fit_image_to_pixel_budget(img, target_w, target_h);
    let composed = if fitted.width() == target_w && fitted.height() == target_h {
        fitted
    } else {
        let mut bg: DynamicImage = ImageBuffer::from_pixel(target_w, target_h, pad).into();
        let x = (target_w.saturating_sub(fitted.width())) / 2;
        let y = (target_h.saturating_sub(fitted.height())) / 2;
        imageops::overlay(&mut bg, &fitted, x as i64, y as i64);
        bg
    };
    flatten_rgba_onto_background(composed, pad)
}

/// Contain-fit into the cell pixel budget, then pad to **exact** `max_w × max_h` with `pad`.
///
/// **ratatui-image contract:** `Picker::new_resize_protocol` builds `ImageSource` with
/// `desired = ceil(bitmap_px / picker.font_size())` in **cells**. The bitmap must therefore use the
/// **same** `FontSize` as the picker, and its pixel size should match the **widget `Rect` × font**
/// (1× budget — not a separate “encode super-res” size), or `desired` will not match the widget
/// and Sixel can leave orphan cells / a black halo.
///
/// Without centered pad, `Resize::Scale` pads again with the picker background (often reads as a
/// second letterbox on Sixel). Matching the panel surface here keeps one consistent matte.
///
/// For large widgets, prefer [`prepare_art_image_for_exact_pixels_contain_centered`] with
/// `inner.width * font.0` × `inner.height * font.1` so `desired` cells match the widget (avoids
/// top-left sixel placement and black bands).
pub fn prepare_art_image_for_rect_contain_centered(
    img: DynamicImage,
    rect: Rect,
    font: FontSize,
    pad: Rgba<u8>,
) -> DynamicImage {
    let (max_w, max_h) = pixel_budget_for_rect(rect, font);
    prepare_art_image_for_exact_pixels_contain_centered(img, max_w, max_h, pad)
}

/// Like [`prepare_art_image_for_rect`], but targets **2×** the nominal strip pixel size (capped),
/// then **resize_exact** to that rectangle. After `crop_center_to_cell_aspect` the bitmap has the
/// same aspect ratio as the cell, so this is a uniform scale — avoids Sixel paths that
/// width-fit then clip vertically inside the widget.
pub fn prepare_art_image_for_strip(img: DynamicImage, rect: Rect, font: FontSize) -> DynamicImage {
    let img = crop_center_to_cell_aspect(img, rect, font);
    let (bw, bh) = pixel_budget_for_rect(rect, font);
    let max_w = (bw.saturating_mul(STRIP_ENCODE_SUPERRES)).min(MAX_ART_EDGE_PX).max(1);
    let max_h = (bh.saturating_mul(STRIP_ENCODE_SUPERRES)).min(MAX_ART_EDGE_PX).max(1);
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 {
        return img;
    }
    img.resize_exact(max_w, max_h, FilterType::Triangle)
}

fn fit_inside(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let wratio = max_w as f64 / w as f64;
    let hratio = max_h as f64 / h as f64;
    let ratio = f64::min(wratio, hratio);
    let nw = ((w as f64 * ratio).round() as u32).max(1);
    let nh = ((h as f64 * ratio).round() as u32).max(1);
    (nw, nh)
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
