/// Runtime theme — resolved ratatui `Color` values built from `ThemeSection`.
///
/// All fields default to the current hardcoded palette so the appearance is
/// identical when no `[theme]` section is present in config.toml.
///
/// Optional `[theme]` colour strings accept:
/// - `#rrggbb` or `rrggbb` (RGB),
/// - terminal indices: `idx:N`, `indexed:N`, `ansi:N`, `color:N`, or `i:N` for `N` in `0..=255`,
/// - `reset` / `inherit` / `default` → inherit the terminal default fg/bg where applicable.
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
        fn apply(opt: Option<&str>, base: Color) -> Color {
            opt.and_then(parse_theme_color).unwrap_or(base)
        }

        let preset = crate::config::theme_preset_from_section(sec);
        let mut theme = match preset {
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
                accent: Color::Rgb(255, 140, 0),
                background: Color::Rgb(26, 26, 26),
                surface: Color::Rgb(22, 22, 22),
                foreground: Color::Rgb(212, 208, 200),
                dimmed: Color::Rgb(90, 88, 88),
                border: Color::Rgb(37, 37, 37),
                border_active: Color::Rgb(58, 58, 58),
                dynamic: false,
            },
            ThemePreset::Dynamic => Self {
                preset,
                accent: Color::Rgb(255, 140, 0),
                background: Color::Rgb(26, 26, 26),
                surface: Color::Rgb(22, 22, 22),
                foreground: Color::Rgb(212, 208, 200),
                dimmed: Color::Rgb(90, 88, 88),
                border: Color::Rgb(37, 37, 37),
                border_active: Color::Rgb(58, 58, 58),
                dynamic: true,
            },
        };

        theme.accent = apply(sec.accent.as_deref(), theme.accent);
        theme.background = apply(sec.background.as_deref(), theme.background);
        theme.surface = apply(sec.surface.as_deref(), theme.surface);
        theme.foreground = apply(sec.foreground.as_deref(), theme.foreground);
        theme.dimmed = apply(sec.dimmed.as_deref(), theme.dimmed);
        theme.border = apply(sec.border.as_deref(), theme.border);
        theme.border_active = apply(sec.border_active.as_deref(), theme.border_active);

        theme
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

/// Parse a theme colour from config: hex RGB, terminal index (`idx:` / `ansi:` / …), or reset.
fn parse_theme_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "reset" | "inherit" | "default" => return Some(Color::Reset),
        _ => {}
    }

    const INDEX_PREFIXES: &[&str] = &["indexed:", "idx:", "ansi:", "color:", "i:"];
    for p in INDEX_PREFIXES {
        if s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p) {
            let rest = s[p.len()..].trim();
            let n: u32 = rest.parse().ok()?;
            return (n <= 255).then_some(Color::Indexed(n as u8));
        }
    }

    parse_hex(s)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_theme_color_hex() {
        assert_eq!(
            parse_theme_color("#76cce0"),
            Some(Color::Rgb(0x76, 0xcc, 0xe0))
        );
        assert_eq!(
            parse_theme_color("76cce0"),
            Some(Color::Rgb(0x76, 0xcc, 0xe0))
        );
    }

    #[test]
    fn parse_theme_color_indexed() {
        assert_eq!(parse_theme_color("idx:2"), Some(Color::Indexed(2)));
        assert_eq!(parse_theme_color("IDX: 14 "), Some(Color::Indexed(14)));
        assert_eq!(parse_theme_color("ansi:255"), Some(Color::Indexed(255)));
        assert_eq!(parse_theme_color("color:0"), Some(Color::Indexed(0)));
        assert_eq!(parse_theme_color("i:6"), Some(Color::Indexed(6)));
        assert_eq!(parse_theme_color("indexed:1"), Some(Color::Indexed(1)));
    }

    #[test]
    fn parse_theme_color_invalid_index() {
        assert_eq!(parse_theme_color("idx:256"), None);
        assert_eq!(parse_theme_color("idx:abc"), None);
    }

    #[test]
    fn terminal_preset_accepts_hex_override() {
        let sec = crate::config::ThemeSection {
            preset: Some("terminal".into()),
            accent: Some("#76cce0".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.preset, ThemePreset::Terminal);
        assert_eq!(t.accent, Color::Rgb(0x76, 0xcc, 0xe0));
        assert_eq!(t.background, Color::Reset);
    }

    #[test]
    fn static_preset_accepts_idx_override() {
        let sec = crate::config::ThemeSection {
            preset: Some("static".into()),
            accent: Some("idx:3".into()),
            ..Default::default()
        };
        let t = Theme::from_section(&sec);
        assert_eq!(t.preset, ThemePreset::Static);
        assert_eq!(t.accent, Color::Indexed(3));
        assert_eq!(t.background, Color::Rgb(26, 26, 26));
    }
}
