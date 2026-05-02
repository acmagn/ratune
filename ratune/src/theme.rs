/// Runtime theme — resolved ratatui `Color` values built from `ThemeSection`.
///
/// All fields default to the current hardcoded palette so the appearance is
/// identical when no `[theme]` section is present in config.toml.
use image::Rgba;
use ratatui::style::Color;

use crate::config::{ThemePreset, ThemeSection};

#[derive(Debug, Clone)]
pub struct Theme {
    pub preset: ThemePreset,
    /// Orange accent: active borders, highlighted items, progress bar fill. (#ff8c00)
    pub accent: Color,
    /// Outer background (status bar, now-playing bar). (#1a1a1a)
    pub background: Color,
    /// Panel backgrounds (browser columns, queue block). (#161616)
    pub surface: Color,
    /// Primary text. (#d4d0c8)
    pub foreground: Color,
    /// Secondary / muted text. (#5a5858)
    pub dimmed: Color,
    /// Inactive pane borders. (#252525)
    pub border: Color,
    /// Active pane borders. (#3a3a3a)
    pub border_active: Color,
    /// Whether to use the dynamic accent extracted from album art.
    pub dynamic: bool,
}

impl Theme {
    pub fn from_section(sec: &ThemeSection) -> Self {
        fn p(opt: Option<&str>, default: Color) -> Color {
            opt.and_then(parse_hex).unwrap_or(default)
        }
        let preset = crate::config::theme_preset_from_section(sec);
        match preset {
            ThemePreset::Terminal => Self {
                preset,
                // Use the terminal's palette / defaults. These indices follow the common ANSI
                // mapping: 0..7 normal colors, 8..15 bright variants.
                //
                // - background/surface: Reset (inherit terminal default bg)
                // - foreground: Reset (inherit terminal default fg)
                // - dimmed/border: bright black / "gray"
                // - accent: blue-ish (4) by convention (matches ncmpcpp-ish defaults), but users
                //   can tune their terminal theme to change what "4" means.
                accent: Color::Indexed(4),
                background: Color::Reset,
                surface: Color::Reset,
                foreground: Color::Reset,
                dimmed: Color::Indexed(8),
                border: Color::Indexed(8),
                border_active: Color::Indexed(4),
                dynamic: false,
            },
            ThemePreset::Static => Self {
                preset,
                accent: p(sec.accent.as_deref(), Color::Rgb(255, 140, 0)),
                background: p(sec.background.as_deref(), Color::Rgb(26, 26, 26)),
                surface: p(sec.surface.as_deref(), Color::Rgb(22, 22, 22)),
                foreground: p(sec.foreground.as_deref(), Color::Rgb(212, 208, 200)),
                dimmed: p(sec.dimmed.as_deref(), Color::Rgb(90, 88, 88)),
                border: p(sec.border.as_deref(), Color::Rgb(37, 37, 37)),
                border_active: p(sec.border_active.as_deref(), Color::Rgb(58, 58, 58)),
                dynamic: false,
            },
            ThemePreset::Dynamic => Self {
                preset,
                accent: p(sec.accent.as_deref(), Color::Rgb(255, 140, 0)),
                background: p(sec.background.as_deref(), Color::Rgb(26, 26, 26)),
                surface: p(sec.surface.as_deref(), Color::Rgb(22, 22, 22)),
                foreground: p(sec.foreground.as_deref(), Color::Rgb(212, 208, 200)),
                dimmed: p(sec.dimmed.as_deref(), Color::Rgb(90, 88, 88)),
                border: p(sec.border.as_deref(), Color::Rgb(37, 37, 37)),
                border_active: p(sec.border_active.as_deref(), Color::Rgb(58, 58, 58)),
                dynamic: true,
            },
        }
    }

    /// Return the accent colour to use for rendering: the dynamic extracted
    /// colour when `self.dynamic` is true and one is provided, else the
    /// static configured accent.
    pub fn effective_accent(&self, dynamic_accent: Option<Color>) -> Color {
        if self.dynamic {
            dynamic_accent.unwrap_or(self.accent)
        } else {
            self.accent
        }
    }
}

/// Parse a 6-digit hex colour string (with or without leading `#`).
/// Solid RGBA for `ratatui-image` padding (Sixel has no transparency — must match panel bg).
pub fn color_to_rgba(c: Color) -> Rgba<u8> {
    match c {
        Color::Rgb(r, g, b) => Rgba([r, g, b, 255]),
        // 16/256-colour terminals: approximate with dark grey (same default as `surface`).
        Color::Indexed(_) | Color::Reset => Rgba([22, 22, 22, 255]),
        // Named ANSI colours — pad with a neutral dark grey (theme is usually Rgb).
        _ => Rgba([22, 22, 22, 255]),
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}
