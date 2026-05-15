//! Spectrum visualizer — braille dot renderer.
//!
//! Uses Unicode braille characters (U+2800–U+28FF) to render frequency bars
//! with 4× the vertical resolution of block characters.  Each terminal cell
//! holds a 2×4 dot grid; only the left column (dots 1-3 and 7) is used so
//! bars are single-column-wide.
//!
//! Braille bit layout for the left column (col 0), top → bottom:
//!   row 0 → bit 0 (value   1)
//!   row 1 → bit 1 (value   2)
//!   row 2 → bit 2 (value   4)
//!   row 3 → bit 6 (value  64)
//!
//! Filling from the bottom, `n` dots lit in one cell:
//!   n=1 → 0x40 → U+2840
//!   n=2 → 0x44 → U+2844
//!   n=3 → 0x46 → U+2846
//!   n=4 → 0x47 → U+2847   (fully filled left column)

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::Theme;

use super::terminal_palette::{stop_to_rgb, GradientRgbCache};

/// Braille codepoints for 0-4 left-column dots filled from the bottom.
/// Index = number of filled dots.
const LEFT_COL: [u32; 5] = [
    0x2800, // 0 dots — blank (skipped in rendering)
    0x2840, // 1 dot  — bit 6
    0x2844, // 2 dots — bits 2,6
    0x2846, // 3 dots — bits 1,2,6
    0x2847, // 4 dots — bits 0,1,2,6  (full left column)
];

/// Render a spectrum visualizer into `area` using braille characters.
///
/// - One bar per column: `num_bars = area.width` (all available columns).
/// - Vertical resolution is 4 dot-rows per cell row.
/// - Only the left dot column is lit, giving thin single-column bars.
/// - The last 2 of 32 computed bands are dropped (noisy high-frequency tail).
///
/// Does nothing if all bands are zero (startup / toggled off).
#[allow(clippy::too_many_arguments)] // Single render entry point for spectrum + waveform modes.
pub fn render_visualizer_ex(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    visualizer_type: &str,
    bands: &[f32],
    waveform: &[f32],
    color_mode: &str,
    colors: &[String],
    accent: Color,
    gradient_rgb_cache: Option<&GradientRgbCache>,
) {
    if area.width == 0 || area.height == 0 || bands.is_empty() {
        // For waveform, bands may be empty; handle below.
    }
    let vtype = visualizer_type.trim().to_lowercase();
    if vtype.as_str() == "wave" {
        render_wave(
            f,
            area,
            theme,
            waveform,
            color_mode,
            colors,
            accent,
            gradient_rgb_cache,
        );
        return;
    }

    if area.width == 0 || area.height == 0 || bands.is_empty() {
        return;
    }
    if bands.iter().all(|&b| b == 0.0) {
        return;
    }

    // Drop the last 2 bands (sparse single-bin high-frequency tail).
    let visible_bands = bands.len().saturating_sub(2);
    if visible_bands == 0 {
        return;
    }

    let num_bars = area.width as usize; // one bar per column
    let max_dots = area.height as usize * 4; // 4 dot-rows per cell row

    for i in 0..num_bars {
        // Map column index evenly across the 30 visible bands.
        let band_idx = i * visible_bands / num_bars;
        let band_val = bands[band_idx].clamp(0.0, 1.0);

        let total_dots = ((band_val * max_dots as f32) as usize).min(max_dots);
        let full_cells = (total_dots / 4).min(area.height as usize);
        let partial_dots = if full_cells < area.height as usize {
            total_dots % 4
        } else {
            0
        };
        let has_partial = partial_dots > 0;
        let bar_rows = full_cells + usize::from(has_partial);

        if bar_rows == 0 {
            continue; // silent bar — leave column blank
        }

        let empty_rows = area.height as usize - bar_rows;

        // Build lines top → bottom for this single-column bar.
        let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);

        for _ in 0..empty_rows {
            lines.push(Line::from(" "));
        }
        if has_partial {
            let ch = char::from_u32(LEFT_COL[partial_dots]).unwrap_or(' ');
            lines.push(Line::from(Span::styled(
                ch.to_string(),
                Style::default().fg(color_for_row(
                    lines.len(),
                    area.height as usize,
                    theme,
                    color_mode,
                    colors,
                    accent,
                    gradient_rgb_cache,
                    i,
                )),
            )));
        }
        for _ in 0..full_cells {
            // U+2847 — all four left-column dots filled
            lines.push(Line::from(Span::styled(
                "\u{2847}",
                Style::default().fg(color_for_row(
                    lines.len(),
                    area.height as usize,
                    theme,
                    color_mode,
                    colors,
                    accent,
                    gradient_rgb_cache,
                    i,
                )),
            )));
        }

        f.render_widget(
            Paragraph::new(lines),
            Rect::new(area.x + i as u16, area.y, 1, area.height),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_wave(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    waveform: &[f32],
    color_mode: &str,
    colors: &[String],
    accent: Color,
    gradient_rgb_cache: Option<&GradientRgbCache>,
) {
    if area.width == 0 || area.height == 0 || waveform.is_empty() {
        return;
    }

    let w = area.width as usize;
    let h = area.height as usize;
    let mid = (h as i32 - 1) / 2;

    // Oscilloscope-style rendering: for each column, compute min/max sample in the bucket
    // and draw the vertical range. This is much more readable than plotting a single
    // sample (which aliases to "random noise" at terminal resolutions).
    let bin = (waveform.len() / w).max(1);

    for x in 0..w {
        let start = x * bin;
        let end = ((x + 1) * bin).min(waveform.len());
        if start >= end {
            continue;
        }
        let mut mn = 1.0f32;
        let mut mx = -1.0f32;
        for &s in &waveform[start..end] {
            mn = mn.min(s);
            mx = mx.max(s);
        }
        mn = mn.clamp(-1.0, 1.0);
        mx = mx.clamp(-1.0, 1.0);

        let y_min = ((mid as f32) - (mn * mid as f32)).round() as i32;
        let y_max = ((mid as f32) - (mx * mid as f32)).round() as i32;
        let y0 = y_min.min(y_max).clamp(0, (h as i32).saturating_sub(1));
        let y1 = y_min.max(y_max).clamp(0, (h as i32).saturating_sub(1));

        for yy in y0..=y1 {
            let color = color_for_row(
                yy as usize,
                h,
                theme,
                color_mode,
                colors,
                accent,
                gradient_rgb_cache,
                x,
            );
            // Keep the waveform readable (min/max envelope) but render with simple,
            // widely-supported glyphs.
            let ch = "•";
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(ch, Style::default().fg(color)))),
                Rect::new(area.x + x as u16, area.y + yy as u16, 1, 1),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn color_for_row(
    row: usize,
    height: usize,
    theme: &Theme,
    color_mode: &str,
    colors: &[String],
    accent: Color,
    gradient_rgb_cache: Option<&GradientRgbCache>,
    dither_col: usize,
) -> Color {
    let mode = color_mode.trim().to_lowercase();
    match mode.as_str() {
        "fixed" => parse_color_spec(
            colors.first().map(|s| s.as_str()).unwrap_or("accent"),
            accent,
        ),
        "gradient_theme" => {
            // Theme palette gradient: dimmed → foreground → accent (top).
            // This stays "in theme" and works well with dynamic accent mode.
            let stops = [theme.dimmed, theme.foreground, accent];
            gradient_color(
                stops.as_slice(),
                row,
                height,
                gradient_rgb_cache,
                dither_col,
            )
        }
        "gradient_height" => {
            if colors.is_empty() {
                return accent;
            }
            if colors.len() == 1 {
                return parse_color_spec(&colors[0], accent);
            }
            let parsed: Vec<Color> = colors.iter().map(|c| parse_color_spec(c, accent)).collect();
            gradient_color(&parsed, row, height, gradient_rgb_cache, dither_col)
        }
        _ => accent, // "accent"
    }
}

fn gradient_color(
    stops: &[Color],
    row: usize,
    height: usize,
    cache: Option<&GradientRgbCache>,
    dither_col: usize,
) -> Color {
    if stops.is_empty() {
        return Color::Reset;
    }
    if stops.len() == 1 || height <= 1 {
        return stops[0];
    }
    let t = (row as f32) / (height.saturating_sub(1) as f32);
    // Map t across segments [0..stops.len-1].
    let segs = (stops.len() - 1) as f32;
    let pos = (t * segs).clamp(0.0, segs);
    let i = pos.floor() as usize;
    let frac = pos - (i as f32);
    let a = stops[i];
    let b = stops[(i + 1).min(stops.len() - 1)];
    lerp_color(a, b, frac, cache, row, dither_col)
}

/// Bayer 4×4 for ordered dither when OSC readback is missing or incomplete.
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

#[inline]
fn dither_two_colors(a: Color, b: Color, frac: f32, row: usize, col: usize) -> Color {
    let m = BAYER4[row % 4][col % 4] as f32;
    if frac * 16.0 > m {
        b
    } else {
        a
    }
}

fn lerp_color(
    a: Color,
    b: Color,
    t: f32,
    cache: Option<&GradientRgbCache>,
    row: usize,
    col: usize,
) -> Color {
    match (stop_to_rgb(a, cache), stop_to_rgb(b, cache)) {
        (Some((ar, ag, ab)), Some((br, bg, bb))) => {
            let lerp_u8 = |x: u8, y: u8| -> u8 {
                (x as f32 + (y as f32 - x as f32) * t)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            Color::Rgb(lerp_u8(ar, br), lerp_u8(ag, bg), lerp_u8(ab, bb))
        }
        _ => dither_two_colors(a, b, t, row, col),
    }
}

fn parse_color_spec(s: &str, accent: Color) -> Color {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("accent") {
        return accent;
    }
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Color::Indexed(idx);
    }
    accent
}
