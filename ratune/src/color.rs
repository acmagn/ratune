use ratatui::style::Color;

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract a dominant vibrant accent colour from raw image bytes.
///
/// Decodes the image, runs palette extraction (up to 5 colours), then picks
/// the colour with the highest HSV saturation that is neither near-black
/// (V < 0.15) nor near-white (V > 0.85).  Returns `None` when no suitable
/// colour is found.
pub fn extract_accent(image_bytes: &[u8]) -> Option<Color> {
    use palette_extract::{
        get_palette_with_options, MaxColors, PixelEncoding, PixelFilter, Quality,
    };

    // Decode to RGBA8.
    let img = image::load_from_memory(image_bytes).ok()?;
    let rgba = img.to_rgba8();
    let pixels: &[u8] = rgba.as_raw();

    let palette = get_palette_with_options(
        pixels,
        PixelEncoding::Rgba,
        Quality::new(10),
        MaxColors::new(5),
        PixelFilter::None,
    );

    palette
        .iter()
        .filter_map(|c| {
            let (_, s, v) = rgb_to_hsv(c.r, c.g, c.b);
            if v < 0.15 || v > 0.85 {
                return None;
            }
            Some((s, c.r, c.g, c.b))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, r, g, b)| ensure_readable(Color::Rgb(r, g, b), 0.55))
}

/// Ensure a colour is readable on a dark background by lifting its OKLab L
/// channel to at least `min_lightness`.  Already-bright colours are unchanged.
pub fn ensure_readable(color: Color, min_lightness: f32) -> Color {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return color,
    };
    let mut lab = rgb_to_oklab(r, g, b);
    if lab[0] >= min_lightness {
        return color;
    }
    lab[0] = min_lightness;
    let (r, g, b) = oklab_to_rgb(lab);
    Color::Rgb(r, g, b)
}

/// Interpolate between two ratatui `Rgb` colours in OKLab space.
///
/// `t` is clamped to [0.0, 1.0].  If either colour is not `Color::Rgb`,
/// returns `a` unchanged.
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab) = match a {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return a,
    };
    let (br, bg, bb) = match b {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return a,
    };

    let la = rgb_to_oklab(ar, ag, ab);
    let lb = rgb_to_oklab(br, bg, bb);

    let lc = [
        la[0] + (lb[0] - la[0]) * t,
        la[1] + (lb[1] - la[1]) * t,
        la[2] + (lb[2] - la[2]) * t,
    ];

    let (r, g, b) = oklab_to_rgb(lc);
    Color::Rgb(r, g, b)
}

// ── Colour-space math ─────────────────────────────────────────────────────────

/// RGB (0–255) → HSV (h: 0–360, s: 0–1, v: 0–1)
fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (h, s, v)
}

/// RGB (0–255) → OKLab [L, a, b]
fn rgb_to_oklab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let r = srgb_to_linear(r as f32 / 255.0);
    let g = srgb_to_linear(g as f32 / 255.0);
    let b = srgb_to_linear(b as f32 / 255.0);

    let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_50 * r + 0.680_699_55 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_70 * b).cbrt();

    [
        0.210_454_26 * l + 0.793_617_78 * m - 0.004_072_05 * s,
        1.977_998_50 * l - 2.428_592_21 * m + 0.450_593_71 * s,
        0.025_904_04 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab [L, a, b] → RGB (0–255)
fn oklab_to_rgb(lab: [f32; 3]) -> (u8, u8, u8) {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_35 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_55 * lab[2];

    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;

    let r =  4.076_741_66 * l - 3.307_711_59 * m + 0.230_969_94 * s;
    let g = -1.268_438_00 * l + 2.609_757_40 * m - 0.341_319_40 * s;
    let b = -0.004_196_09 * l - 0.703_418_61 * m + 1.707_614_70 * s;

    (
        (linear_to_srgb(r) * 255.0).clamp(0.0, 255.0) as u8,
        (linear_to_srgb(g) * 255.0).clamp(0.0, 255.0) as u8,
        (linear_to_srgb(b) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(x: f32) -> f32 {
    let x = x.max(0.0); // guard against tiny negatives from floating-point rounding
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}
